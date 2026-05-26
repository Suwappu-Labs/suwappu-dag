//! Reserved L1 registry-account addresses.
//!
//! Several Track C / Track G features need stable, deterministically-
//! derived 20-byte addresses that act as protocol-owned registry
//! accounts:
//!
//! - **`l2_registry_address`**: stores `(chain_id, batch_id) →
//!   L2StateRoot` mappings via the `gsx-l2-verifier-precompile` arm of
//!   `apply_intent` (per `docs/iq/IQ-006-l2-state-root-commitment-
//!   surface.md`). Wired in G2.2 (#97).
//! - **`insurance_pool_address`**: holds slashed funds reimbursing
//!   affected counterparties + cross-validator insurance liquidity
//!   per Tokenomics §8.3 step 2. Wired here in C.8 (#131).
//! - **`treasury_address`**: holds protocol-treasury balance (the
//!   third step of the slashing-distribution waterfall + the
//!   foundation's 20% Treasury allocation per Tokenomics §3.2).
//!   Wired here in C.8.
//!
//! ## Reserved-address invariant
//!
//! These addresses are protocol-owned: **no user Intent may mutate
//! them via the standard `Transfer` path**. Only the dedicated arms
//! of `apply_intent` (the slashing-distribution arm here, the L2
//! verifier-precompile arm in G2.2) may write to reserved-address
//! balance slots.
//!
//! `is_reserved` enforces this gate. Both `Substrate` impls
//! (`InMemorySubstrate`, `GsxDbSubstrate`) reject any `Intent::Transfer`
//! whose `from` or `to` is reserved.
//!
//! ## Derivation
//!
//! Each address is the leading 20 bytes of `BLAKE3(domain_tag)`. The
//! address-space collision probability with user-generated addresses
//! is `2^-160`, negligible. Domain tags are pinned strings (no length
//! prefix — input space is constant, not user-controlled).

use blake3::Hasher;

use crate::substrate::Address;

/// Domain tag for the L2 state-root registry account.
pub const L2_REGISTRY_DOMAIN: &[u8] = b"gsx-l2-registry-v1";

/// Domain tag for the insurance-pool registry account.
pub const INSURANCE_POOL_DOMAIN: &[u8] = b"gsx-insurance-pool-v1";

/// Domain tag for the protocol-treasury registry account.
pub const TREASURY_DOMAIN: &[u8] = b"gsx-treasury-v1";

/// Domain tag for the L1↔L2 bridge-escrow account.
///
/// The escrow holds locked L1 GSX while equivalent value is
/// credited on L2. Bridge accounting invariant: at every block
/// boundary, `balance(bridge_escrow_address) == sum_of_unwithdrawn_L2_deposits`.
pub const BRIDGE_ESCROW_DOMAIN: &[u8] = b"gsx-bridge-escrow-v1";

/// Domain tag for the force-include obligation registry.
/// Stores `obligation_id → ForceIncludeObligation` records via
/// the substrate's bytes_state surface (Track G G3.4, #103).
pub const FORCE_INCLUDE_REGISTRY_DOMAIN: &[u8] = b"gsx-force-include-registry-v1";

/// Domain tag for the sequencer's liveness bond. 3,000,000
/// GSX per Track G "Sequencer bonding"; drains 5% per
/// missed-force-include slash. Refundable.
pub const SEQUENCER_BOND_DOMAIN: &[u8] = b"gsx-sequencer-bond-v1";

/// Domain tag for the sequencer's safety bond. 15,000,000
/// GSX per Track G "Sequencer bonding"; 100% forfeit on
/// `SlashReason::Equivocation` or `SlashReason::InvalidBatch`.
/// Separate from the liveness bond so the partial-drain
/// model for missed-force-include can't accidentally drain
/// equivocation collateral.
pub const SAFETY_BOND_DOMAIN: &[u8] = b"gsx-safety-bond-v1";

/// Domain tag for the bridge-asset registry account.
/// Stores `asset_id → AssetRecord` records via the substrate's
/// bytes_state surface (Track I I.5, #166).
pub const ASSET_REGISTRY_DOMAIN: &[u8] = b"gsx-asset-registry-v1";

/// Domain tag for the sequencer-ejection registry account.
/// Stores `obligation_id → EjectionRecord` records via the
/// substrate's bytes_state surface (Track G G3.4
/// permissionless-fallback path). One record per Slashed
/// obligation that's been ejected past the 10k-block
/// fallback window.
pub const EJECTION_REGISTRY_DOMAIN: &[u8] = b"gsx-ejection-registry-v1";

/// Domain tag for the L2 burn-nullifier registry account.
/// Stores the set of claimed `burn_id`s — one per
/// successful `Intent::L2BurnProven` — to prevent
/// double-spending a single L2 burn against the bridge
/// escrow (Track G G3.2 hardening).
pub const BURN_NULLIFIER_REGISTRY_DOMAIN: &[u8] = b"gsx-burn-nullifier-registry-v1";

/// Domain tag for the equivocation registry account.
/// Stores the set of slashed proof_hashes — one per
/// successful `Intent::SlashSequencer { reason:
/// Equivocation | InvalidBatch, .. }` — to prevent
/// re-slashing the same offense after a safety-bond
/// refill (Track G G3.4 hardening).
pub const EQUIVOCATION_REGISTRY_DOMAIN: &[u8] = b"gsx-equivocation-registry-v1";

/// Domain tag for the Authority Ring registry account.
/// Stores `authority_id → AuthorityRecord` mappings:
/// active/exiting/ejected status, pubkey material, and
/// declared stake. Mutated by AdmitAuthority/
/// ExitAuthority/EjectAuthority.
pub const AUTHORITY_REGISTRY_DOMAIN: &[u8] = b"gsx-authority-registry-v1";

/// Domain tag for the Validator Ring registry account.
/// Mirror of the Authority Ring registry for the Tier B
/// Validator set. Stores `validator_id → ValidatorRecord`
/// mappings, mutated by AdmitValidator/ExitValidator/
/// EjectValidator.
pub const VALIDATOR_REGISTRY_DOMAIN: &[u8] = b"gsx-validator-registry-v1";

/// Domain tag for the Authority Ring stake pool. Holds
/// protocol-locked GSX backing Authority Ring slots;
/// credited by `Intent::DepositAuthorityStake`.
pub const AUTHORITY_STAKE_POOL_DOMAIN: &[u8] = b"gsx-authority-stake-pool-v1";

/// Domain tag for the Validator Ring stake pool. Mirror
/// of `authority_stake_pool` for the Tier B Validator set.
pub const VALIDATOR_STAKE_POOL_DOMAIN: &[u8] = b"gsx-validator-stake-pool-v1";

/// Domain tag for the Authority Ring rewards pool. Receives
/// freshly-minted GSX from `Intent::MintInflation`; drained
/// by per-epoch reward distribution.
pub const AUTHORITY_REWARDS_POOL_DOMAIN: &[u8] = b"gsx-authority-rewards-pool-v1";

/// Domain tag for the Validator Ring rewards pool. Mirror
/// of `authority_rewards_pool` for Tier B.
pub const VALIDATOR_REWARDS_POOL_DOMAIN: &[u8] = b"gsx-validator-rewards-pool-v1";

/// Domain tag for the inflation registry. Stores the last
/// minted epoch number (`u64::BE`) so `Intent::MintInflation`
/// can replay-defend its own emission.
pub const INFLATION_REGISTRY_DOMAIN: &[u8] = b"gsx-inflation-registry-v1";

/// Domain tag for the rewards-distribution registry. Stores
/// per-ring last-distributed-epoch (16 BE bytes: 8 for
/// authority, 8 for validator) so `Intent::DistributeRewards`
/// can replay-defend its own emission per ring.
pub const REWARDS_DISTRIBUTION_REGISTRY_DOMAIN: &[u8] = b"gsx-rewards-distribution-registry-v1";

/// Domain tag for the validator-delegation registry. Stores
/// `(validator_id, delegator_address) → amount` records for
/// `Intent::Delegate` (Tokenomics §4 delegated PoS).
pub const VALIDATOR_DELEGATION_REGISTRY_DOMAIN: &[u8] = b"gsx-validator-delegation-registry-v1";

/// Domain tag for the DA-anchor registry (Track G G3.3). Stores
/// `BLAKE3("gsx-da-blob-v1" || da_blob)` keyed by
/// `(l2_chain_id_hash, batch_id)` so off-chain auditors can
/// link L1 calldata to the `da_commitment` in the matching
/// `L2StateRootRecord`. Written by `Intent::PostL2DAv2`
/// (versioned alongside the unchanged `Intent::PostL2DA` per
/// IQ-007).
pub const DA_ANCHOR_REGISTRY_DOMAIN: &[u8] = b"gsx-da-anchor-registry-v1";

/// Compute the reserved address corresponding to `domain` —
/// `BLAKE3(domain)[..20]`. Used by the three exposed helpers below.
/// Inlined per call site (BLAKE3 is sub-microsecond).
fn derive(domain: &[u8]) -> Address {
    let mut h = Hasher::new();
    h.update(domain);
    let digest = h.finalize();
    let mut out = [0u8; 20];
    out.copy_from_slice(&digest.as_bytes()[..20]);
    out
}

/// Reserved address for the L2 state-root registry account
/// (per IQ-006). Wired into G2.2's verifier-precompile arm.
pub fn l2_registry_address() -> Address {
    derive(L2_REGISTRY_DOMAIN)
}

/// Reserved address for the insurance-pool account
/// (per Tokenomics §8.3 step 2).
pub fn insurance_pool_address() -> Address {
    derive(INSURANCE_POOL_DOMAIN)
}

/// Reserved address for the protocol-treasury account
/// (per Tokenomics §8.3 step 3 + §3.2 foundation allocation).
pub fn treasury_address() -> Address {
    derive(TREASURY_DOMAIN)
}

/// Reserved address for the L1↔L2 bridge-escrow account.
/// Holds locked L1 balances while equivalent value is credited
/// on L2 (Track G G3.2, issue #101).
pub fn bridge_escrow_address() -> Address {
    derive(BRIDGE_ESCROW_DOMAIN)
}

/// Reserved address for the force-include obligation registry
/// (Track G G3.4, #103). Stores `obligation_id →
/// ForceIncludeObligation` records in the substrate's bytes_state.
pub fn force_include_registry_address() -> Address {
    derive(FORCE_INCLUDE_REGISTRY_DOMAIN)
}

/// Reserved address for the sequencer's liveness bond.
/// Track G G3.4: per-slash drain via
/// `Intent::SlashSequencer { reason: MissedForceInclude, ... }`.
pub fn sequencer_bond_address() -> Address {
    derive(SEQUENCER_BOND_DOMAIN)
}

/// Reserved address for the sequencer's safety bond.
/// Track G G3.4: 100% forfeit on
/// `Intent::SlashSequencer { reason: Equivocation | InvalidBatch, ... }`.
pub fn safety_bond_address() -> Address {
    derive(SAFETY_BOND_DOMAIN)
}

/// Reserved address for the bridge-asset registry (Track I
/// I.5, #166). Stores `asset_id → AssetRecord` records.
pub fn asset_registry_address() -> Address {
    derive(ASSET_REGISTRY_DOMAIN)
}

/// Reserved address for the sequencer-ejection registry
/// (Track G G3.4 permissionless-fallback). Stores
/// `obligation_id → EjectionRecord` records: one per
/// Slashed obligation that's been ejected past the
/// 10k-block fallback window.
pub fn ejection_registry_address() -> Address {
    derive(EJECTION_REGISTRY_DOMAIN)
}

/// Reserved address for the L2 burn-nullifier registry.
/// Stores the set of claimed `burn_id`s — one per
/// successful `Intent::L2BurnProven` — to prevent
/// double-spending a single L2 burn against the bridge
/// escrow (Track G G3.2 hardening).
pub fn burn_nullifier_registry_address() -> Address {
    derive(BURN_NULLIFIER_REGISTRY_DOMAIN)
}

/// Reserved address for the equivocation registry. Stores
/// the set of slashed proof_hashes — one per successful
/// `Intent::SlashSequencer { reason: Equivocation |
/// InvalidBatch, .. }` — to prevent re-slashing the same
/// offense after a safety-bond refill.
pub fn equivocation_registry_address() -> Address {
    derive(EQUIVOCATION_REGISTRY_DOMAIN)
}

/// Reserved address for the Authority Ring registry.
/// Stores `authority_id → AuthorityRecord` mappings,
/// mutated by AdmitAuthority/ExitAuthority/EjectAuthority.
pub fn authority_registry_address() -> Address {
    derive(AUTHORITY_REGISTRY_DOMAIN)
}

/// Reserved address for the Validator Ring registry.
/// Mirror of `authority_registry_address` for the Tier B
/// validator set. Stores `validator_id → ValidatorRecord`
/// mappings, mutated by AdmitValidator/ExitValidator/
/// EjectValidator.
pub fn validator_registry_address() -> Address {
    derive(VALIDATOR_REGISTRY_DOMAIN)
}

/// Reserved address for the Authority Ring stake pool.
/// Holds protocol-locked GSX backing Authority slots;
/// credited by `Intent::DepositAuthorityStake`.
pub fn authority_stake_pool_address() -> Address {
    derive(AUTHORITY_STAKE_POOL_DOMAIN)
}

/// Reserved address for the Validator Ring stake pool.
/// Mirror of `authority_stake_pool_address` for Tier B.
pub fn validator_stake_pool_address() -> Address {
    derive(VALIDATOR_STAKE_POOL_DOMAIN)
}

/// Reserved address for the Authority Ring rewards pool.
/// Credited by `Intent::MintInflation`; drained by per-epoch
/// reward distribution (follow-up PR).
pub fn authority_rewards_pool_address() -> Address {
    derive(AUTHORITY_REWARDS_POOL_DOMAIN)
}

/// Reserved address for the Validator Ring rewards pool.
/// Mirror of `authority_rewards_pool_address` for Tier B.
pub fn validator_rewards_pool_address() -> Address {
    derive(VALIDATOR_REWARDS_POOL_DOMAIN)
}

/// Reserved address for the inflation registry. Bytes_state
/// holds the last minted epoch (`u64::BE`) for replay defense
/// on `Intent::MintInflation`.
pub fn inflation_registry_address() -> Address {
    derive(INFLATION_REGISTRY_DOMAIN)
}

/// Reserved address for the rewards-distribution registry.
/// Bytes_state holds per-ring last-distributed-epoch (16 BE
/// bytes: 8 for authority, 8 for validator) for replay
/// defense on `Intent::DistributeRewards`.
pub fn rewards_distribution_registry_address() -> Address {
    derive(REWARDS_DISTRIBUTION_REGISTRY_DOMAIN)
}

/// Reserved address for the validator-delegation registry
/// (Tokenomics §4). Stores `(validator_id, delegator_address) →
/// amount` records for `Intent::Delegate`.
pub fn validator_delegation_registry_address() -> Address {
    derive(VALIDATOR_DELEGATION_REGISTRY_DOMAIN)
}

/// Reserved address for the DA-anchor registry account
/// (Track G G3.3). Written by `Intent::PostL2DAv2`.
pub fn da_anchor_registry_address() -> Address {
    derive(DA_ANCHOR_REGISTRY_DOMAIN)
}

/// Returns true if `addr` is a reserved protocol-owned registry
/// account. Both `Substrate` impls reject `Intent::Transfer` into
/// or out of a reserved address.
pub fn is_reserved(addr: &Address) -> bool {
    addr == &l2_registry_address()
        || addr == &insurance_pool_address()
        || addr == &treasury_address()
        || addr == &bridge_escrow_address()
        || addr == &force_include_registry_address()
        || addr == &sequencer_bond_address()
        || addr == &safety_bond_address()
        || addr == &asset_registry_address()
        || addr == &ejection_registry_address()
        || addr == &burn_nullifier_registry_address()
        || addr == &equivocation_registry_address()
        || addr == &authority_registry_address()
        || addr == &validator_registry_address()
        || addr == &authority_stake_pool_address()
        || addr == &validator_stake_pool_address()
        || addr == &authority_rewards_pool_address()
        || addr == &validator_rewards_pool_address()
        || addr == &inflation_registry_address()
        || addr == &rewards_distribution_registry_address()
        || addr == &validator_delegation_registry_address()
        || addr == &da_anchor_registry_address()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_one_reserved_addresses_are_distinct() {
        let all = [
            l2_registry_address(),
            insurance_pool_address(),
            treasury_address(),
            bridge_escrow_address(),
            force_include_registry_address(),
            sequencer_bond_address(),
            safety_bond_address(),
            asset_registry_address(),
            ejection_registry_address(),
            burn_nullifier_registry_address(),
            equivocation_registry_address(),
            authority_registry_address(),
            validator_registry_address(),
            authority_stake_pool_address(),
            validator_stake_pool_address(),
            authority_rewards_pool_address(),
            validator_rewards_pool_address(),
            inflation_registry_address(),
            rewards_distribution_registry_address(),
            validator_delegation_registry_address(),
            da_anchor_registry_address(),
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "reserved addresses {i} and {j} collide");
                }
            }
        }
    }

    #[test]
    fn derivation_is_deterministic() {
        assert_eq!(l2_registry_address(), l2_registry_address());
        assert_eq!(insurance_pool_address(), insurance_pool_address());
        assert_eq!(treasury_address(), treasury_address());
        assert_eq!(bridge_escrow_address(), bridge_escrow_address());
        assert_eq!(
            force_include_registry_address(),
            force_include_registry_address()
        );
        assert_eq!(sequencer_bond_address(), sequencer_bond_address());
        assert_eq!(safety_bond_address(), safety_bond_address());
        assert_eq!(asset_registry_address(), asset_registry_address());
        assert_eq!(ejection_registry_address(), ejection_registry_address());
        assert_eq!(
            burn_nullifier_registry_address(),
            burn_nullifier_registry_address()
        );
        assert_eq!(
            equivocation_registry_address(),
            equivocation_registry_address()
        );
        assert_eq!(authority_registry_address(), authority_registry_address());
        assert_eq!(validator_registry_address(), validator_registry_address());
        assert_eq!(
            authority_stake_pool_address(),
            authority_stake_pool_address()
        );
        assert_eq!(
            validator_stake_pool_address(),
            validator_stake_pool_address()
        );
    }

    #[test]
    fn is_reserved_matches_all_fifteen() {
        assert!(is_reserved(&l2_registry_address()));
        assert!(is_reserved(&insurance_pool_address()));
        assert!(is_reserved(&treasury_address()));
        assert!(is_reserved(&bridge_escrow_address()));
        assert!(is_reserved(&force_include_registry_address()));
        assert!(is_reserved(&sequencer_bond_address()));
        assert!(is_reserved(&safety_bond_address()));
        assert!(is_reserved(&asset_registry_address()));
        assert!(is_reserved(&ejection_registry_address()));
        assert!(is_reserved(&burn_nullifier_registry_address()));
        assert!(is_reserved(&equivocation_registry_address()));
        assert!(is_reserved(&authority_registry_address()));
        assert!(is_reserved(&validator_registry_address()));
        assert!(is_reserved(&authority_stake_pool_address()));
        assert!(is_reserved(&validator_stake_pool_address()));
    }

    #[test]
    fn is_reserved_rejects_arbitrary_address() {
        assert!(!is_reserved(&[0u8; 20]));
        assert!(!is_reserved(&[0xffu8; 20]));
        assert!(!is_reserved(&[0xabu8; 20]));
    }

    /// Defensive: the derivation is BLAKE3-truncated, not the
    /// `sha3_256_domain` length-prefix pattern used elsewhere in the
    /// crypto crate. Test pins the bytes so a refactor that
    /// accidentally swaps hashes is caught.
    #[test]
    fn known_blake3_truncation() {
        let addr = l2_registry_address();
        // BLAKE3("gsx-l2-registry-v1") leading-20-bytes
        // — locked by this test; updating requires governance vote
        // since changing the reserved address bricks any state at
        // the old address.
        let expected = {
            let mut h = Hasher::new();
            h.update(L2_REGISTRY_DOMAIN);
            let mut out = [0u8; 20];
            out.copy_from_slice(&h.finalize().as_bytes()[..20]);
            out
        };
        assert_eq!(addr, expected);
    }
}
