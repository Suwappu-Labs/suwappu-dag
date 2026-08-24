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

/// Compute the leaf hash for one distribution entry.
pub fn leaf_hash(index: u32, account: &Address, amount: Balance) -> [u8; 32] {
    let mut data = Vec::with_capacity(4 + 20 + 16);
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
pub fn build_root(entries: &[(Address, Balance)]) -> [u8; 32] {
    assert!(!entries.is_empty(), "empty distribution");
    let mut level: Vec<[u8; 32]> = entries
        .iter()
        .enumerate()
        .map(|(i, (account, amount))| leaf_hash(i as u32, account, *amount))
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
pub fn build_proof(entries: &[(Address, Balance)], index: u32) -> Vec<[u8; 32]> {
    let mut level: Vec<[u8; 32]> = entries
        .iter()
        .enumerate()
        .map(|(i, (account, amount))| leaf_hash(i as u32, account, *amount))
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
    use super::*;

    fn addr(b: u8) -> Address {
        [b; 20]
    }

    #[test]
    fn proof_roundtrip_all_leaves() {
        // 5 entries → odd-promotion tree, exercises both chunk arms.
        let entries: Vec<(Address, Balance)> =
            (0u8..5).map(|i| (addr(i), 100 + i as u128)).collect();
        let root = build_root(&entries);
        for (i, (account, amount)) in entries.iter().enumerate() {
            let leaf = leaf_hash(i as u32, account, *amount);
            let proof = build_proof(&entries, i as u32);
            assert!(verify_proof(&root, &leaf, &proof), "leaf {i}");
        }
    }

    #[test]
    fn wrong_amount_index_or_account_fails() {
        let entries: Vec<(Address, Balance)> = (0u8..4).map(|i| (addr(i), 100)).collect();
        let root = build_root(&entries);
        let proof = build_proof(&entries, 2);
        assert!(verify_proof(&root, &leaf_hash(2, &addr(2), 100), &proof));
        assert!(!verify_proof(&root, &leaf_hash(2, &addr(2), 101), &proof));
        assert!(!verify_proof(&root, &leaf_hash(3, &addr(2), 100), &proof));
        assert!(!verify_proof(&root, &leaf_hash(2, &addr(9), 100), &proof));
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
}
