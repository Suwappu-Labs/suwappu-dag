//! TGE Merkle claim distribution — the `MerkleDistributor` pattern.
//!
//! Ports the mechanism live Ethereum airdrops run on — Uniswap's
//! `MerkleDistributor` (verified on mainnet at
//! `0x090D4613473dEE047c3f2706764f49E0821D256e`, deployed 2020-09-16)
//! and Arbitrum's `TokenDistributor` are the reference deployments —
//! onto the chain's own primitives: a governance-set Merkle root per
//! TGE pool, permissionless proof-carrying claims, and a claimed-index
//! bitmap, all stored in the pool's own `bytes_state` record (the pool
//! addresses are already reserved, so no new registry account is
//! needed).
//!
//! Divergences from the Solidity original, and why:
//!
//! - **SHA3-256 with domain separation instead of keccak256.** The
//!   chain's canonical hash surface is SHA3-256 (`suwappu-crypto`,
//!   PQ-conservative invariant). Leaves and internal nodes get
//!   *distinct* domain tags, which structurally prevents the classic
//!   leaf/internal-node second-preimage confusion that Solidity
//!   distributors avoid by ad-hoc double-hashing (the OpenZeppelin
//!   recommendation).
//! - **Sorted-pair internal nodes** (OpenZeppelin `MerkleProof`
//!   convention): proofs carry no direction bits.
//! - **Rounds instead of one-contract-per-drop.** On Ethereum each
//!   airdrop deploys a fresh distributor; here, rotating the root
//!   (`Intent::SetTgeRoot`) bumps a round counter and resets the
//!   claimed bitmap — the analogue for the Seasons pool's recurring
//!   drops. The pool's remaining balance is the only budget; a round
//!   cannot pay out what the pool does not hold.
//!
//! Record layout at the pool's `bytes_state` address:
//! `round: u32 BE ‖ merkle_root: 32 bytes ‖ claimed_bitmap: N bytes`.

use suwappu_crypto::hash::sha3_256_domain;

use crate::substrate::{Address, Balance};

/// Domain tag for distribution leaves: `(index ‖ account ‖ amount)`.
pub const TGE_CLAIM_LEAF_DOMAIN: &[u8] = b"suwappu-tge-claim-leaf-v1";

/// Domain tag for internal Merkle nodes: `(min(a,b) ‖ max(a,b))`.
pub const TGE_CLAIM_NODE_DOMAIN: &[u8] = b"suwappu-tge-claim-node-v1";

/// Highest leaf index a claim may carry (2^22 ≈ 4.19M leaves —
/// a ~512 KiB bitmap at full width). Without a bound, a single
/// claim at `u32::MAX` would force a 512 MiB bitmap record that is
/// re-hashed into `state_root` on every block: a permanent
/// chain-wide DoS. Distribution tooling MUST assign positional
/// indices `0..n`.
pub const MAX_TGE_CLAIM_INDEX: u32 = 1 << 22;

/// Longest accepted Merkle proof. Depth 32 covers 2^32 leaves —
/// far above [`MAX_TGE_CLAIM_INDEX`] — while bounding per-intent
/// hash work.
pub const MAX_TGE_PROOF_LEN: usize = 32;

/// Compute the leaf hash for one distribution entry.
///
/// The leaf commits the POOL and the ROUND in addition to
/// `(index, account, amount)`: a proof is only valid for the one
/// pool and the one distribution round its tree was built for, so
/// a root built for one pool cannot be replayed onto another, a
/// round-N proof cannot pay out under round N+1's bitmap reset,
/// and a claim intent straddling a rotation fails loudly instead
/// of being evaluated against the wrong tree.
pub fn leaf_hash(
    pool: &Address,
    round: u32,
    index: u32,
    account: &Address,
    amount: Balance,
) -> [u8; 32] {
    let mut data = Vec::with_capacity(20 + 4 + 4 + 20 + 16);
    data.extend_from_slice(pool);
    data.extend_from_slice(&round.to_be_bytes());
    data.extend_from_slice(&index.to_be_bytes());
    data.extend_from_slice(account);
    data.extend_from_slice(&amount.to_be_bytes());
    sha3_256_domain(TGE_CLAIM_LEAF_DOMAIN, &data)
}

/// Combine two child hashes into a parent, sorted-pair convention.
pub fn node_hash(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(lo);
    data.extend_from_slice(hi);
    sha3_256_domain(TGE_CLAIM_NODE_DOMAIN, &data)
}

/// Verify a Merkle proof from `leaf` up to `root`.
pub fn verify_proof(root: &[u8; 32], leaf: &[u8; 32], proof: &[[u8; 32]]) -> bool {
    let mut acc = *leaf;
    for sibling in proof {
        acc = node_hash(&acc, sibling);
    }
    &acc == root
}

/// One TGE pool's distribution record: current round, that round's
/// Merkle root, and the round's claimed-index bitmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgeDistribution {
    /// Monotonic round counter; bumped by each `SetTgeRoot`.
    pub round: u32,
    /// The active round's Merkle root.
    pub merkle_root: [u8; 32],
    /// Claimed-index bitmap for the active round (bit `i` = leaf
    /// index `i` claimed). Grows on demand.
    pub claimed: Vec<u8>,
}

impl TgeDistribution {
    /// Start a fresh round with `root` (round 1 when `prev` is None).
    pub fn new_round(prev: Option<&TgeDistribution>, root: [u8; 32]) -> Self {
        Self {
            round: prev.map(|p| p.round).unwrap_or(0).wrapping_add(1),
            merkle_root: root,
            claimed: Vec::new(),
        }
    }

    /// Whether leaf `index` is already claimed this round.
    pub fn is_claimed(&self, index: u32) -> bool {
        let byte = (index / 8) as usize;
        let bit = index % 8;
        self.claimed
            .get(byte)
            .map(|b| b & (1u8 << bit) != 0)
            .unwrap_or(false)
    }

    /// Mark leaf `index` claimed.
    pub fn set_claimed(&mut self, index: u32) {
        let byte = (index / 8) as usize;
        let bit = index % 8;
        if self.claimed.len() <= byte {
            self.claimed.resize(byte + 1, 0);
        }
        self.claimed[byte] |= 1u8 << bit;
    }

    /// Encode to the bytes_state record layout.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 32 + self.claimed.len());
        out.extend_from_slice(&self.round.to_be_bytes());
        out.extend_from_slice(&self.merkle_root);
        out.extend_from_slice(&self.claimed);
        out
    }

    /// Decode from the bytes_state record layout. `None` on any
    /// malformed record (callers fail closed as state corruption).
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 36 {
            return None;
        }
        let round = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
        let mut merkle_root = [0u8; 32];
        merkle_root.copy_from_slice(&bytes[4..36]);
        Some(Self {
            round,
            merkle_root,
            claimed: bytes[36..].to_vec(),
        })
    }
}

/// Build the Merkle root over `(index, account, amount)` entries,
/// where entry `i` gets leaf index `i`. Odd nodes promote (the
/// Uniswap distributor's convention). Test/tooling helper — the chain
/// only ever *verifies*; roots are computed off-chain over the
/// published distribution set.
pub fn build_root(pool: &Address, round: u32, entries: &[(Address, Balance)]) -> [u8; 32] {
    assert!(!entries.is_empty(), "empty distribution");
    let mut level: Vec<[u8; 32]> = entries
        .iter()
        .enumerate()
        .map(|(i, (account, amount))| leaf_hash(pool, round, i as u32, account, *amount))
        .collect();
    while level.len() > 1 {
        level = level
            .chunks(2)
            .map(|pair| {
                if pair.len() == 2 {
                    node_hash(&pair[0], &pair[1])
                } else {
                    pair[0]
                }
            })
            .collect();
    }
    level[0]
}

/// Build the proof for leaf `index` under the same conventions as
/// [`build_root`]. Test/tooling helper.
pub fn build_proof(
    pool: &Address,
    round: u32,
    entries: &[(Address, Balance)],
    index: u32,
) -> Vec<[u8; 32]> {
    let mut level: Vec<[u8; 32]> = entries
        .iter()
        .enumerate()
        .map(|(i, (account, amount))| leaf_hash(pool, round, i as u32, account, *amount))
        .collect();
    let mut idx = index as usize;
    let mut proof = Vec::new();
    while level.len() > 1 {
        let sibling = idx ^ 1;
        if sibling < level.len() {
            proof.push(level[sibling]);
        }
        level = level
            .chunks(2)
            .map(|pair| {
                if pair.len() == 2 {
                    node_hash(&pair[0], &pair[1])
                } else {
                    pair[0]
                }
            })
            .collect();
        idx /= 2;
    }
    proof
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn addr(b: u8) -> Address {
        [b; 20]
    }

    const POOL: Address = [0xAA; 20];

    #[test]
    fn proof_roundtrip_all_leaves() {
        // 5 entries → odd-promotion tree, exercises both chunk arms.
        let entries: Vec<(Address, Balance)> =
            (0u8..5).map(|i| (addr(i), 100 + i as u128)).collect();
        let root = build_root(&POOL, 1, &entries);
        for (i, (account, amount)) in entries.iter().enumerate() {
            let leaf = leaf_hash(&POOL, 1, i as u32, account, *amount);
            let proof = build_proof(&POOL, 1, &entries, i as u32);
            assert!(verify_proof(&root, &leaf, &proof), "leaf {i}");
        }
    }

    #[test]
    fn wrong_amount_index_account_pool_or_round_fails() {
        let entries: Vec<(Address, Balance)> = (0u8..4).map(|i| (addr(i), 100)).collect();
        let root = build_root(&POOL, 1, &entries);
        let proof = build_proof(&POOL, 1, &entries, 2);
        assert!(verify_proof(
            &root,
            &leaf_hash(&POOL, 1, 2, &addr(2), 100),
            &proof
        ));
        assert!(!verify_proof(
            &root,
            &leaf_hash(&POOL, 1, 2, &addr(2), 101),
            &proof
        ));
        assert!(!verify_proof(
            &root,
            &leaf_hash(&POOL, 1, 3, &addr(2), 100),
            &proof
        ));
        assert!(!verify_proof(
            &root,
            &leaf_hash(&POOL, 1, 2, &addr(9), 100),
            &proof
        ));
        // Pool and round are committed in the leaf: the same tree
        // cannot pay from another pool or survive a round rotation.
        assert!(!verify_proof(
            &root,
            &leaf_hash(&[0xBB; 20], 1, 2, &addr(2), 100),
            &proof
        ));
        assert!(!verify_proof(
            &root,
            &leaf_hash(&POOL, 2, 2, &addr(2), 100),
            &proof
        ));
    }

    /// A valid proof plus one extra sibling must fail (extension),
    /// a truncated proof must fail, and an empty proof must fail
    /// against any multi-leaf root.
    #[test]
    fn proof_extension_truncation_and_empty_fail() {
        let entries: Vec<(Address, Balance)> = (0u8..4).map(|i| (addr(i), 100)).collect();
        let root = build_root(&POOL, 1, &entries);
        let leaf = leaf_hash(&POOL, 1, 0, &addr(0), 100);
        let good = build_proof(&POOL, 1, &entries, 0);
        assert!(verify_proof(&root, &leaf, &good));

        let mut extended = good.clone();
        extended.push([0x77; 32]);
        assert!(!verify_proof(&root, &leaf, &extended));

        let truncated = &good[..good.len() - 1];
        assert!(!verify_proof(&root, &leaf, truncated));

        assert!(!verify_proof(&root, &leaf, &[]));
    }

    /// Single-entry tree: the root IS the leaf hash and the empty
    /// proof is the (only) valid proof — the one shape where the
    /// empty-proof branch is legitimately live.
    #[test]
    fn single_entry_tree_claims_with_empty_proof() {
        let entries: Vec<(Address, Balance)> = vec![(addr(1), 42)];
        let root = build_root(&POOL, 1, &entries);
        assert_eq!(root, leaf_hash(&POOL, 1, 0, &addr(1), 42));
        assert!(verify_proof(
            &root,
            &leaf_hash(&POOL, 1, 0, &addr(1), 42),
            &[]
        ));
        assert!(!verify_proof(
            &root,
            &leaf_hash(&POOL, 1, 0, &addr(2), 42),
            &[]
        ));
    }

    /// A leaf hash can never masquerade as an internal node (and vice
    /// versa): distinct domain tags make the classic second-preimage
    /// splice structurally impossible.
    #[test]
    fn leaf_and_node_domains_disjoint() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let node = node_hash(&a, &b);
        let mut as_leaf_input = Vec::new();
        as_leaf_input.extend_from_slice(&a);
        as_leaf_input.extend_from_slice(&b);
        // Hashing the same 64 bytes under the leaf domain must differ.
        assert_ne!(node, sha3_256_domain(TGE_CLAIM_LEAF_DOMAIN, &as_leaf_input));
    }

    #[test]
    fn bitmap_and_codec_roundtrip() {
        let mut d = TgeDistribution::new_round(None, [7u8; 32]);
        assert_eq!(d.round, 1);
        assert!(!d.is_claimed(0));
        assert!(!d.is_claimed(1000));
        d.set_claimed(0);
        d.set_claimed(9);
        d.set_claimed(1000);
        assert!(d.is_claimed(0) && d.is_claimed(9) && d.is_claimed(1000));
        assert!(!d.is_claimed(1) && !d.is_claimed(999));
        let decoded = TgeDistribution::decode(&d.encode()).unwrap();
        assert_eq!(decoded, d);
        // Rotation bumps round and clears claims.
        let next = TgeDistribution::new_round(Some(&d), [8u8; 32]);
        assert_eq!(next.round, 2);
        assert!(!next.is_claimed(0));
        // Malformed records fail closed.
        assert!(TgeDistribution::decode(&[0u8; 35]).is_none());
    }

    proptest! {
        /// Every leaf of every tree size in 1..=64 verifies with its
        /// own proof and fails with a forged amount — the sprint-gate
        /// property for the verifier guarding the pre-mine.
        #[test]
        fn prop_roundtrip_and_forgery(n in 1usize..=64, round in 1u32..=8) {
            let entries: Vec<(Address, Balance)> = (0..n)
                .map(|i| ([i as u8; 20], 1_000 + i as u128))
                .collect();
            let root = build_root(&POOL, round, &entries);
            for (i, (account, amount)) in entries.iter().enumerate() {
                let proof = build_proof(&POOL, round, &entries, i as u32);
                prop_assert!(proof.len() <= MAX_TGE_PROOF_LEN);
                let leaf = leaf_hash(&POOL, round, i as u32, account, *amount);
                prop_assert!(verify_proof(&root, &leaf, &proof));
                let forged = leaf_hash(&POOL, round, i as u32, account, *amount + 1);
                prop_assert!(!verify_proof(&root, &forged, &proof));
            }
        }

        /// Proof extension never verifies: appending any sibling to a
        /// valid proof breaks it.
        #[test]
        fn prop_extension_never_verifies(
            n in 2usize..=32,
            pos in 0usize..32,
            extra in prop::array::uniform32(0u8..)
        ) {
            let pos = pos % n;
            let entries: Vec<(Address, Balance)> = (0..n)
                .map(|i| ([i as u8; 20], 500 + i as u128))
                .collect();
            let root = build_root(&POOL, 1, &entries);
            let leaf = leaf_hash(&POOL, 1, pos as u32, &entries[pos].0, entries[pos].1);
            let mut proof = build_proof(&POOL, 1, &entries, pos as u32);
            proof.push(extra);
            prop_assert!(!verify_proof(&root, &leaf, &proof));
        }
    }
}
