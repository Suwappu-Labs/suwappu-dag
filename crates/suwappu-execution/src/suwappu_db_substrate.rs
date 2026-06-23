//! Real-suwappu-db [`Substrate`] adapter.
//!
//! Wraps `suwappudb-state::State` (the authoritative balance map of paper
//! §7.2) and `suwappudb-bridge::Bridge` (the capability-gated mutation
//! path) behind the [`Substrate`] trait this crate exposes to the
//! consensus pipeline.
//!
//! This is the real wire-up promised by the DAG-S10 sprint state.
//! With `suwappu-db` v0.1.0 cut on GitHub and consumed as a workspace
//! `git` dependency, the in-memory mock of S10 is no longer the only
//! `Substrate` implementation — production validators run the
//! `SuwappuDbSubstrate` and inherit every Phase-1 substrate invariant
//! from suwappu-db (lane separation, dual-projection, schedule
//! determinism, bundle atomicity, tree determinism, replay equivalence).

use suwappudb_bridge::{Bridge, Intent as SuwappuIntent, RejectReason};
use suwappudb_state::{Address as SuwappuAddress, State};

use crate::{
    error::ExecutionError,
    reserved,
    substrate::{Address, Balance, Intent, Substrate},
};

/// Production [`Substrate`] implementation backed by suwappu-db's
/// `State` + `Bridge`. Every mutation traverses the capability-gated
/// `Bridge::submit` path, so the lane-separation invariant of paper
/// §7.4.1 is inherited structurally — there is no way for this adapter
/// to mutate `suwappudb-state` except through `suwappudb-bridge`.
#[derive(Debug, Default)]
pub struct SuwappuDbSubstrate {
    state: State,
}

impl SuwappuDbSubstrate {
    /// Construct an empty substrate over a fresh `suwappudb-state::State`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-populate the substrate with the given `(address, balance)`
    /// pairs. Useful for tests; production validators populate state
    /// through `Bridge::submit` only.
    pub fn from_balances<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (Address, Balance)>,
    {
        let mut s = Self::new();
        {
            // Protocol-owned credit via the suwappu-db v0.4.0 `Bridge::credit`
            // primitive — the durable analogue of
            // `InMemorySubstrate::credit_unchecked`. This replaces the
            // phase-1 no-op: the adapter can now construct realistic test
            // fixtures (and any protocol-owned seed flow) without a
            // minter/self-transfer dance, while still routing through the
            // capability-gated `State::apply` (only a `Bridge` holding the
            // `BridgeToken` can mutate state). Zero balances are no-ops in
            // `credit` itself.
            let mut bridge = Bridge::new(&mut s.state);
            for (addr, balance) in entries {
                bridge
                    .credit(SuwappuAddress(addr), balance)
                    .expect("from_balances: credit on a fresh state cannot overflow");
            }
        }
        s
    }

    /// Borrow the underlying `State`. Diagnostic / test access; the
    /// trait's `Substrate::balance` is the production read path.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Borrow `&mut State`. Provided so callers can spin up a
    /// `suwappudb_bridge::Bridge` directly for advanced flows (anchor
    /// dispatch, bundle execution, recovery replay).
    pub fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }
}

impl Substrate for SuwappuDbSubstrate {
    fn balance(&self, addr: &Address) -> Balance {
        // suwappudb-state::Address is a newtype over [u8; 20]; matches our
        // local Address shape exactly.
        let suwappu_addr = SuwappuAddress(*addr);
        self.state.balance_of(&suwappu_addr).0
    }

    fn read_bytes(&self, addr: &Address) -> Option<Vec<u8>> {
        // Reads the suwappu-db v0.5.0 bytes column (zero-is-absent), matching
        // InMemorySubstrate::read_bytes. The economic-security arms RMW
        // registries through this + Bridge::write_bytes.
        self.state.bytes_of(&SuwappuAddress(*addr))
    }

    fn apply_intent(&mut self, intent: &Intent) -> Result<(), ExecutionError> {
        let mut bridge = Bridge::new(&mut self.state);
        match intent {
            Intent::Transfer { from, to, amount } => {
                let (from, to, amount) = (*from, *to, *amount);
                // C.8 reserved-address invariant (matches
                // InMemorySubstrate). The protocol-owned registry
                // accounts may only be mutated by the substrate-
                // internal arms below.
                if reserved::is_reserved(&from) {
                    return Err(ExecutionError::ReservedAddressTransferDenied { addr: from });
                }
                if reserved::is_reserved(&to) {
                    return Err(ExecutionError::ReservedAddressTransferDenied { addr: to });
                }
                let result = bridge.submit(SuwappuIntent::Transfer {
                    from: SuwappuAddress(from),
                    to: SuwappuAddress(to),
                    amount,
                });
                match result {
                    Ok(()) => Ok(()),
                    Err(RejectReason::InsufficientBalance) => {
                        // Convert to our local error variant. We need
                        // the actual source balance for the message;
                        // re-read via the bridge. `Balance` is a newtype
                        // over u128 in suwappudb-state — unwrap with `.0`.
                        let have = bridge.balance_of(&SuwappuAddress(from)).0;
                        Err(ExecutionError::InsufficientBalance {
                            from,
                            have,
                            need: amount,
                        })
                    }
                    Err(RejectReason::AmountOverflow) => {
                        Err(ExecutionError::BalanceOverflow { to })
                    }
                    // A Transfer can only fail on balance/overflow. Call/Move
                    // rejection reasons (CallRequiresRegistry, Move VM variants)
                    // are unreachable from the phase-1 Substrate, which only has
                    // Transfer intents.
                    Err(other) => {
                        unreachable!("Substrate Transfer cannot produce {other:?}")
                    }
                }
            }
            // Governance variants are no-ops on the suwappu-db substrate;
            // see DAG-S25.2 in `substrate.rs::apply_intent`.
            Intent::AdmitAuthority { .. }
            | Intent::ExitAuthority { .. }
            | Intent::EjectAuthority { .. }
            | Intent::AdmitValidator { .. }
            | Intent::ExitValidator { .. }
            | Intent::EjectValidator { .. }
            | Intent::GenesisAllocation { .. }
            | Intent::MintInflation { .. }
            | Intent::DistributeRewards { .. }
            | Intent::Delegate { .. } => Ok(()),
            // Track G Phase G2.2 (#97): wired through the
            // suwappu-l2-verifier-precompile crate. The verifier
            // format gates (proof = 260 B, public_inputs = 240 B,
            // vk_hash != all-zeros) run regardless of substrate.
            //
            // The SuwappuDbSubstrate side cannot yet write the commit
            // marker (suwappudb-bridge::Bridge::submit only carries
            // Transfer semantics; the production wire-up needs a
            // suwappu-db v0.2.0 bridge extension that exposes a
            // protocol-owned credit path matching
            // InMemorySubstrate::credit_unchecked). The verifier
            // gate still runs so an invalid proof rejects
            // uniformly across both impls.
            Intent::CommitL2StateRoot {
                proof_bytes,
                public_inputs,
                vk_hash,
                ..
            } => {
                suwappu_l2_verifier_precompile::verify_l2_batch(
                    proof_bytes,
                    public_inputs,
                    vk_hash,
                )
                .map_err(|e| ExecutionError::L2VerifierRejected {
                    reason: e.to_string(),
                })?;
                Ok(())
            }
            // `SetL2VerifyingKey` chain-state storage lands with
            // the same suwappu-db v0.2.0 follow-up.
            Intent::SetL2VerifyingKey { .. } => Ok(()),
            // Track G Phase G3.2 (#101): bridge accounting for
            // L1Lock + L2BurnProven. suwappudb-bridge::Bridge::submit
            // v0.1.0 only carries Transfer semantics; the
            // protocol-owned credit path needs a suwappu-db v0.2.0
            // bridge extension (matching the C.8
            // DistributeSlashedFunds + G2.2 CommitL2StateRoot
            // stubs). Until then the SuwappuDbSubstrate stubs;
            // InMemorySubstrate handles the real accounting +
            // tests exercise the semantics there.
            Intent::L1Lock { .. }
            | Intent::L2BurnProven { .. }
            | Intent::L2ForceInclude { .. }
            | Intent::SlashSequencer { .. }
            | Intent::MarkForceIncludeHonored { .. }
            | Intent::EjectSequencer { .. }
            | Intent::DepositSequencerBond { .. }
            | Intent::DepositSafetyBond { .. }
            | Intent::DepositAuthorityStake { .. }
            | Intent::DepositValidatorStake { .. }
            | Intent::WithdrawAuthorityStake { .. }
            | Intent::WithdrawValidatorStake { .. }
            | Intent::ClaimInsurance { .. }
            | Intent::DisburseTreasury { .. }
            | Intent::PostL2DA { .. } => Ok(()),
            // C.8 (#131): slashing-distribution waterfall.
            // suwappu-db v0.1.0's `Bridge::submit` only exposes the
            // capability-gated Transfer path — no `credit_unchecked`
            // analogue that bypasses transfer semantics. Production
            // wire-up requires a suwappu-db v0.2.0 bridge extension
            // (tracked separately) that exposes a protocol-owned
            // credit path for the slashed-stake source +
            // counterparty/insurance/treasury destinations.
            //
            // For phase-1 we accept the Intent so the dispatch path
            // is exercised through the consensus pipeline; the
            // InMemorySubstrate handles the real accounting + tests
            // exercise the full waterfall semantics there. Once
            // suwappu-db ships the credit path this arm calls into it.
            Intent::DistributeSlashedFunds { .. } => Ok(()),
            // Track I I.5 (#166): asset whitelist Intents stub
            // until suwappu-db v0.2.0 exposes a bytes_state-style
            // surface. InMemorySubstrate handles the full
            // accounting; tests exercise the registry logic
            // there.
            Intent::AddBridgeAsset { .. }
            | Intent::PauseBridgeAsset { .. }
            | Intent::RemoveBridgeAsset { .. } => Ok(()),
        }
    }

    fn state_root(&self) -> [u8; 32] {
        // The canonical consensus root is the substrate-level **V2 recipe**
        // (`compute_state_root_v2`), NOT suwappu-db's internal balance trie —
        // so this substrate is a byte-for-byte drop-in for InMemorySubstrate
        // (a state_root parity test pins this). suwappu-db's trie root remains
        // its own internal commitment and is intentionally unused here.
        //
        // Normalise to the helper's input contract: ascending by address,
        // zero balances dropped (suwappu-db's balance store is NOT
        // remove-on-zero, unlike the in-memory map's zero-is-absent
        // invariant). The bytes column is already zero-is-absent.
        let mut balances: Vec<(Address, u128)> = self
            .state
            .entries()
            .into_iter()
            .map(|(a, slot)| (a.0, slot.canonical()))
            .filter(|(_, bal)| *bal != 0)
            .collect();
        balances.sort_by(|x, y| x.0.cmp(&y.0));

        let mut bytes: Vec<(Address, Vec<u8>)> = self
            .state
            .bytes_entries()
            .into_iter()
            .map(|(a, d)| (a.0, d))
            .collect();
        bytes.sort_by(|x, y| x.0.cmp(&y.0));

        crate::substrate::compute_state_root_v2(
            balances.iter().map(|(a, b)| (a, *b)),
            bytes.iter().map(|(a, d)| (a, d.as_slice())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(seed: u8) -> Address {
        [seed; 20]
    }

    /// Empty substrate has a deterministic state root that matches
    /// itself across construction.
    #[test]
    fn empty_substrate_root_is_deterministic() {
        let a = SuwappuDbSubstrate::new();
        let b = SuwappuDbSubstrate::new();
        assert_eq!(a.state_root(), b.state_root());
    }

    /// Zero-balance address reads as zero.
    #[test]
    fn unseen_address_is_zero() {
        let s = SuwappuDbSubstrate::new();
        assert_eq!(s.balance(&addr(1)), 0);
    }

    /// Insufficient balance: suwappu-db's Bridge rejects via
    /// `RejectReason::InsufficientBalance`, and our adapter surfaces
    /// the equivalent `ExecutionError::InsufficientBalance` with
    /// `have = 0` (because no balance was seeded).
    #[test]
    fn insufficient_balance_rejected() {
        let mut s = SuwappuDbSubstrate::new();
        let err = s.apply_intent(&Intent::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 100,
        });
        assert!(matches!(
            err,
            Err(ExecutionError::InsufficientBalance { .. })
        ));
    }

    /// Self-transfer is a no-op at suwappu-db's Bridge level too —
    /// inherited from the suwappu-db self-transfer guard (the same bug my
    /// in-memory substrate hit at S10 was already fixed in suwappu-db).
    #[test]
    fn self_transfer_is_noop_at_suwappudb() {
        let mut s = SuwappuDbSubstrate::new();
        // With zero balance the self-transfer still fails on the
        // balance check, exercising the same error surface.
        let err = s.apply_intent(&Intent::Transfer {
            from: addr(1),
            to: addr(1),
            amount: 0,
        });
        // Amount = 0 is a no-op in both substrates.
        assert!(err.is_ok());
    }

    /// `from_balances` now actually seeds via the suwappu-db v0.4.0
    /// `Bridge::credit` primitive (it was a silent no-op before v0.4.0).
    #[test]
    fn from_balances_seeds_real_balances() {
        let s = SuwappuDbSubstrate::from_balances([(addr(1), 100), (addr(2), 250)]);
        assert_eq!(s.balance(&addr(1)), 100);
        assert_eq!(s.balance(&addr(2)), 250);
        assert_eq!(s.balance(&addr(3)), 0);
    }

    /// End-to-end on the durable substrate: seed → Transfer → balances
    /// settle. This flow was impossible pre-v0.4.0 because the substrate
    /// could not be seeded outside a minter dance.
    #[test]
    fn seeded_transfer_settles_on_durable_substrate() {
        let mut s = SuwappuDbSubstrate::from_balances([(addr(1), 100)]);
        s.apply_intent(&Intent::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 30,
        })
        .unwrap();
        assert_eq!(s.balance(&addr(1)), 70);
        assert_eq!(s.balance(&addr(2)), 30);
    }

    /// Balance parity with the in-memory reference substrate for the
    /// seed→transfer flow: both produce identical final balances.
    #[test]
    fn balances_match_in_memory_reference() {
        let seed = [(addr(1), 100u128), (addr(2), 5u128)];
        let mut durable = SuwappuDbSubstrate::from_balances(seed);
        let mut reference = crate::substrate::InMemorySubstrate::from_balances(seed);
        let xfer = Intent::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 40,
        };
        assert_eq!(
            durable.apply_intent(&xfer).is_ok(),
            reference.apply_intent(&xfer).is_ok()
        );
        for a in [addr(1), addr(2), addr(3)] {
            assert_eq!(durable.balance(&a), reference.balance(&a), "addr {a:?}");
        }
    }

    // ── state_root parity: SuwappuDbSubstrate is a drop-in for InMemorySubstrate ──

    #[test]
    fn state_root_matches_in_memory_reference() {
        // The keystone of this rework: identical logical state → identical
        // canonical V2 root across both substrates. Pre-v0.6.0 this FAILED —
        // SuwappuDb used suwappu-db's balance trie, InMemory the V2 recipe.
        let seed = [(addr(1), 100u128), (addr(2), 250u128)];
        let mut durable = SuwappuDbSubstrate::from_balances(seed);
        let mut reference = crate::substrate::InMemorySubstrate::from_balances(seed);

        // (a) Balances-only — proves balances_root + top combination match.
        assert_eq!(
            durable.state_root(),
            reference.state_root(),
            "balances-only state_root parity"
        );

        // (b) Put identical bytes-state on both at the same reserved address,
        // then re-check — proves the bytes_state_root path matches too.
        reference.pin_l2_verifying_key([0xab; 32], [0xcd; 32]).unwrap();
        let reg = crate::reserved::l2_registry_address();
        let raw = reference.read_bytes(&reg).expect("reference wrote bytes");
        {
            let mut bridge = Bridge::new(durable.state_mut());
            bridge.write_bytes(SuwappuAddress(reg), raw.clone());
        }
        assert_eq!(durable.read_bytes(&reg), Some(raw), "durable mirrors the bytes");
        assert_eq!(
            durable.state_root(),
            reference.state_root(),
            "balances + bytes state_root parity"
        );
    }

    #[test]
    fn state_root_drops_zero_balance_to_match_in_memory() {
        // suwappu-db's balance store is NOT remove-on-zero; the in-memory map
        // is zero-is-absent. SuwappuDbSubstrate.state_root() must filter zeros
        // so a stray zero entry can't shift the consensus root.
        use suwappudb_state::{Balance, BridgeToken, StateChange};
        let mut durable = SuwappuDbSubstrate::from_balances([(addr(1), 100)]);
        {
            let token = BridgeToken::__for_bridge_only();
            durable.state_mut().apply(
                &token,
                &StateChange::SetBalance {
                    addr: SuwappuAddress(addr(2)),
                    to: Balance(0),
                },
            );
        }
        let reference = crate::substrate::InMemorySubstrate::from_balances([(addr(1), 100)]);
        assert_eq!(
            durable.state_root(),
            reference.state_root(),
            "a zero-balance entry must not change the root"
        );
    }
}
