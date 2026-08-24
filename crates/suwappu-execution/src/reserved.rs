//! Reserved L1 registry-account addresses.
//!
//! Several Track C / Track G features need stable, deterministically-
//! derived 20-byte addresses that act as protocol-owned registry
//! accounts:
//!
//! - **`l2_registry_address`**: stores `(chain_id, batch_id) →
//!   L2StateRoot` mappings via the `suwappu-l2-verifier-precompile` arm of
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
//! (`InMemorySubstrate`, `SuwappuDbSubstrate`) reject any `Intent::Transfer`
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
pub const L2_REGISTRY_DOMAIN: &[u8] = b"suwappu-l2-registry-v1";

/// Domain tag for the insurance-pool registry account.
pub const INSURANCE_POOL_DOMAIN: &[u8] = b"suwappu-insurance-pool-v1";

/// Domain tag for the protocol-treasury registry account.
pub const TREASURY_DOMAIN: &[u8] = b"suwappu-treasury-v1";

/// Domain tag for the L1↔L2 bridge-escrow account.
///
/// The escrow holds locked L1 SUWAPPU while equivalent value is
/// credited on L2. Bridge accounting invariant: at every block
/// boundary, `balance(bridge_escrow_address) == sum_of_unwithdrawn_L2_deposits`.
pub const BRIDGE_ESCROW_DOMAIN: &[u8] = b"suwappu-bridge-escrow-v1";

/// Domain tag for the force-include obligation registry.
/// Stores `obligation_id → ForceIncludeObligation` records via
/// the substrate's bytes_state surface (Track G G3.4, #103).
pub const FORCE_INCLUDE_REGISTRY_DOMAIN: &[u8] = b"suwappu-force-include-registry-v1";

/// Domain tag for the sequencer's liveness bond. 3,000,000
/// SUWAPPU per Track G "Sequencer bonding"; drains 5% per
/// missed-force-include slash. Refundable.
pub const SEQUENCER_BOND_DOMAIN: &[u8] = b"suwappu-sequencer-bond-v1";

/// Domain tag for the sequencer's safety bond. 15,000,000
/// SUWAPPU per Track G "Sequencer bonding"; 100% forfeit on
/// `SlashReason::Equivocation` or `SlashReason::InvalidBatch`.
/// Separate from the liveness bond so the partial-drain
/// model for missed-force-include can't accidentally drain
/// equivocation collateral.
pub const SAFETY_BOND_DOMAIN: &[u8] = b"suwappu-safety-bond-v1";

/// Domain tag for the bridge-asset registry account.
/// Stores `asset_id → AssetRecord` records via the substrate's
/// bytes_state surface (Track I I.5, #166).
pub const ASSET_REGISTRY_DOMAIN: &[u8] = b"suwappu-asset-registry-v1";

/// Domain tag for the sequencer-ejection registry account.
/// Stores `obligation_id → EjectionRecord` records via the
/// substrate's bytes_state surface (Track G G3.4
/// permissionless-fallback path). One record per Slashed
/// obligation that's been ejected past the 10k-block
/// fallback window.
pub const EJECTION_REGISTRY_DOMAIN: &[u8] = b"suwappu-ejection-registry-v1";

/// Domain tag for the L2 burn-nullifier registry account.
/// Stores the set of claimed `burn_id`s — one per
/// successful `Intent::L2BurnProven` — to prevent
/// double-spending a single L2 burn against the bridge
/// escrow (Track G G3.2 hardening).
pub const BURN_NULLIFIER_REGISTRY_DOMAIN: &[u8] = b"suwappu-burn-nullifier-registry-v1";

/// Domain tag for the equivocation registry account.
/// Stores the set of slashed proof_hashes — one per
/// successful `Intent::SlashSequencer { reason:
/// Equivocation | InvalidBatch, .. }` — to prevent
/// re-slashing the same offense after a safety-bond
/// refill (Track G G3.4 hardening).
pub const EQUIVOCATION_REGISTRY_DOMAIN: &[u8] = b"suwappu-equivocation-registry-v1";

/// Domain tag for the Authority Ring registry account.
/// Stores `authority_id → AuthorityRecord` mappings:
/// active/exiting/ejected status, pubkey material, and
/// declared stake. Mutated by AdmitAuthority/
/// ExitAuthority/EjectAuthority.
pub const AUTHORITY_REGISTRY_DOMAIN: &[u8] = b"suwappu-authority-registry-v1";

/// Domain tag for the Validator Ring registry account.
/// Mirror of the Authority Ring registry for the Tier B
/// Validator set. Stores `validator_id → ValidatorRecord`
/// mappings, mutated by AdmitValidator/ExitValidator/
/// EjectValidator.
pub const VALIDATOR_REGISTRY_DOMAIN: &[u8] = b"suwappu-validator-registry-v1";

/// Domain tag for the Authority Ring stake pool. Holds
/// protocol-locked SUWAPPU backing Authority Ring slots;
/// credited by `Intent::DepositAuthorityStake`.
pub const AUTHORITY_STAKE_POOL_DOMAIN: &[u8] = b"suwappu-authority-stake-pool-v1";

/// Domain tag for the Validator Ring stake pool. Mirror
/// of `authority_stake_pool` for the Tier B Validator set.
pub const VALIDATOR_STAKE_POOL_DOMAIN: &[u8] = b"suwappu-validator-stake-pool-v1";

/// Domain tag for the Authority Ring rewards pool. Receives
/// freshly-minted SUWAPPU from `Intent::MintInflation`; drained
/// by per-epoch reward distribution.
pub const AUTHORITY_REWARDS_POOL_DOMAIN: &[u8] = b"suwappu-authority-rewards-pool-v1";

/// Domain tag for the Validator Ring rewards pool. Mirror
/// of `authority_rewards_pool` for Tier B.
pub const VALIDATOR_REWARDS_POOL_DOMAIN: &[u8] = b"suwappu-validator-rewards-pool-v1";

/// Domain tag for the inflation registry. Stores the last
/// minted epoch number (`u64::BE`) so `Intent::MintInflation`
/// can replay-defend its own emission.
pub const INFLATION_REGISTRY_DOMAIN: &[u8] = b"suwappu-inflation-registry-v1";

/// Domain tag for the rewards-distribution registry. Stores
/// per-ring last-distributed-epoch (16 BE bytes: 8 for
/// authority, 8 for validator) so `Intent::DistributeRewards`
/// can replay-defend its own emission per ring.
pub const REWARDS_DISTRIBUTION_REGISTRY_DOMAIN: &[u8] = b"suwappu-rewards-distribution-registry-v1";

/// Domain tag for the validator-delegation registry. Stores
/// `(validator_id, delegator_address) → amount` records for
/// `Intent::Delegate` (Tokenomics §4 delegated PoS).
pub const VALIDATOR_DELEGATION_REGISTRY_DOMAIN: &[u8] = b"suwappu-validator-delegation-registry-v1";

/// Domain tag for the TGE fair-launch distribution pool.
/// Holds the open-public-distribution share of the genesis
/// pre-mine until the TGE claim path drains it (fair-launch
/// tokenomics, `docs/whitepaper/TOKENOMICS.md` §2.1). Ledger
/// entry: `scripts/tge/allocations.toml`.
pub const TGE_FAIR_LAUNCH_POOL_DOMAIN: &[u8] = b"suwappu-tge-fair-launch-pool-v1";

/// Domain tag for the TGE Seasons-program pool. Funds the
/// suwappubot Seasons usage-reward schedule (30% of supply,
/// `TOKENOMICS.md` §2.2) from genesis.
pub const TGE_SEASONS_POOL_DOMAIN: &[u8] = b"suwappu-tge-seasons-pool-v1";

/// Domain tag for the TGE testnet-points pool. Funds the
/// testnet validator points→token conversion
/// (`docs/testnet/POINTS.md`; 8%-of-supply ceiling,
/// `TOKENOMICS.md` §2.4).
pub const TGE_TESTNET_POINTS_POOL_DOMAIN: &[u8] = b"suwappu-tge-testnet-points-pool-v1";

/// Domain tag for the supply registry. Bytes_state holds
/// `max_supply ‖ issued` (32 BE bytes: two u128s). Written once
/// at genesis when the manifest sets `max_supply_suwappu`;
/// `Intent::MintInflation` fail-closes against it so total
/// issuance can never exceed the genesis-committed max supply.
/// Absent (legacy manifests) = no cap.
pub const SUPPLY_REGISTRY_DOMAIN: &[u8] = b"suwappu-supply-registry-v1";

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
/// Holds protocol-locked SUWAPPU backing Authority slots;
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

/// Reserved address for the TGE fair-launch distribution pool
/// (`TOKENOMICS.md` §2.1). Credited by genesis prebalances;
/// drained only by the TGE claim path (pre-TGE gap tracked in
/// `docs/testnet/LAUNCH-STATUS.md`).
pub fn tge_fair_launch_pool_address() -> Address {
    derive(TGE_FAIR_LAUNCH_POOL_DOMAIN)
}

/// Reserved address for the TGE Seasons-program pool
/// (`TOKENOMICS.md` §2.2).
pub fn tge_seasons_pool_address() -> Address {
    derive(TGE_SEASONS_POOL_DOMAIN)
}

/// Reserved address for the TGE testnet-points pool
/// (`TOKENOMICS.md` §2.4).
pub fn tge_testnet_points_pool_address() -> Address {
    derive(TGE_TESTNET_POINTS_POOL_DOMAIN)
}

/// Reserved address for the supply registry. Bytes_state holds
/// `max_supply ‖ issued` (32 BE bytes: two u128s); see
/// `SUPPLY_REGISTRY_DOMAIN`.
pub fn supply_registry_address() -> Address {
    derive(SUPPLY_REGISTRY_DOMAIN)
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
        || addr == &tge_fair_launch_pool_address()
        || addr == &tge_seasons_pool_address()
        || addr == &tge_testnet_points_pool_address()
        || addr == &supply_registry_address()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_reserved_addresses_are_distinct() {
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
            tge_fair_launch_pool_address(),
            tge_seasons_pool_address(),
            tge_testnet_points_pool_address(),
            supply_registry_address(),
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
        assert!(is_reserved(&tge_fair_launch_pool_address()));
        assert!(is_reserved(&tge_seasons_pool_address()));
        assert!(is_reserved(&tge_testnet_points_pool_address()));
        assert!(is_reserved(&supply_registry_address()));
    }

    /// Cross-language parity pin: the TGE pool addresses published in
    /// `scripts/tge/allocations.toml` (derived by the Python tooling
    /// via `blake3(domain_tag)[:20]`) must byte-match this crate's
    /// derivation. A drift here silently pre-mines into addresses the
    /// chain does not protect. Updating any pinned hex requires
    /// updating the published ledger + `docs/whitepaper/TOKENOMICS.md`
    /// in the same commit.
    #[test]
    fn tge_pool_addresses_match_published_ledger() {
        let cases: [(&str, Address); 4] = [
            (
                "f9e86688d4afeeff73b01067237e5529149905f0",
                tge_fair_launch_pool_address(),
            ),
            (
                "ae360caae624555b7fc6a2b7a96def76780d9e43",
                tge_seasons_pool_address(),
            ),
            (
                "9e28b89b1c3b49a75f3b782e0ac9ee5919b340f9",
                tge_testnet_points_pool_address(),
            ),
            // Staking-rewards pre-mine lands directly in the existing
            // per-ring rewards pools (drained by DistributeRewards):
            (
                "1148457e50ba9ee1b9197e98dd0efc096063a50c",
                authority_rewards_pool_address(),
            ),
        ];
        for (expected_hex, addr) in cases {
            assert_eq!(hex::encode(addr), expected_hex);
        }
        assert_eq!(
            hex::encode(validator_rewards_pool_address()),
            "ef9bd42745ebdf4dbcb15e21426a670efbc407f5"
        );
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
        // BLAKE3("suwappu-l2-registry-v1") leading-20-bytes
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
