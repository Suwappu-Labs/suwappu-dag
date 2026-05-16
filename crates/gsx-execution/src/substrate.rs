//! Substrate trait + in-memory adapter.
//!
//! The `Substrate` trait is the API surface that the block executor
//! consumes. The phase-1 `InMemorySubstrate` is a minimal balance-map
//! implementation that mirrors `gsx-db`'s `BalanceStore` interface,
//! sufficient for the DAG-S10 exit gate. When the gsx-db v0.1.0 tag is
//! cut on GitHub, the real wrapper will be `GsxDbSubstrate`, a thin
//! adapter over `gsxdb-bridge::BlockExecutor` and `gsxdb-state::State`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ExecutionError;

/// 20-byte EVM-compatible address. Phase-1 phase uses the raw 20 bytes
/// directly; the 32-byte Move address shape lands with the IQ-4 address-
/// shape policy in gsx-db's launch-readiness sprint.
pub type Address = [u8; 20];

/// Balance type. `u128` matches the canonical `gsx-db::BalanceSlot` storage.
pub type Balance = u128;

/// A state-mutating intent. Carries balance transfers plus
/// Phase G validator-set governance actions. Governance variants
/// (`AdmitAuthority` / `ExitAuthority` / `EjectAuthority`) do not
/// mutate the substrate's balance state — they are picked up by the
/// daemon and queued for atomic application at the next epoch
/// boundary (DAG-S25.3).
///
/// `Copy` was dropped in S25.2 to accommodate variable-size pubkey
/// material. Existing pattern matches now bind by reference.
///
/// C4 hardening: `#[non_exhaustive]` ensures external crates that
/// match on `Intent` must include a wildcard arm, so adding a new
/// variant in a future protocol revision (Phase G3/G4 governance
/// operations, fast-path intents, LTP-bound intents, etc.) is a
/// non-breaking change for SDK consumers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Intent {
    /// Transfer `amount` from `from` to `to`.
    Transfer {
        /// Source address.
        from: Address,
        /// Destination address.
        to: Address,
        /// Transfer amount.
        amount: Balance,
    },
    /// Admit a new Authority Ring member, applied at the next epoch
    /// boundary. Stake gates whether the candidate makes the active
    /// set (selection logic lands in S25.3); pubkey material is the
    /// canonical ML-DSA-65 + BLS12-381 binding.
    AdmitAuthority {
        /// Candidate authority id (zero-indexed slot the caller wants).
        authority_id: u32,
        /// Stake the candidate is locking. Must be ≥ floor.
        stake_gsx: u64,
        /// ML-DSA-65 public key (1952 B canonical).
        mldsa_public_key: Vec<u8>,
        /// BLS12-381 G1 public key (48 B compressed).
        bls_public_key: Vec<u8>,
    },
    /// Voluntary withdrawal. Applied at the next epoch boundary.
    /// MVP has no cooling-off period — that's a Phase G follow-on.
    ExitAuthority {
        /// Authority id to remove from the active set.
        authority_id: u32,
    },
    /// Ejection on confirmed equivocation (paper Invariant 5).
    /// Carries a reference to the proof transaction so the slashing
    /// pipeline can audit. 100% bonded stake forfeit.
    EjectAuthority {
        /// Authority id being ejected.
        authority_id: u32,
        /// Reference to the equivocation proof (cert hash or
        /// EquivocationProof commitment).
        proof_ref: [u8; 32],
    },
    /// Commit a per-batch L2 state root to the L1 chain. Submitted by
    /// the L2 prover after successfully proving an L2 batch with SP1
    /// (Track G Phase G2 + G4). The verifier-precompile arm in
    /// `apply_intent` validates the Groth16 BN254 proof against
    /// `vk_hash` + the chain-state's `aggregation_vk_hash`, then
    /// writes the new state root into the reserved registry account
    /// `gsx_dag_l2_registry` (per
    /// `docs/iq/IQ-006-l2-state-root-commitment-surface.md`).
    ///
    /// **Phase 1 (this PR / G2.1)**: only the variant is added.
    /// The verifier-precompile body lands in G2.2 (#97); until then
    /// the arm is a stub that accepts the Intent without state effect.
    CommitL2StateRoot {
        /// Monotonic per-L2-chain batch identifier.
        batch_id: u64,
        /// EVM MPT root produced by the L2 STM after applying the
        /// batch's tx list (per Open Item #8 EVM flip).
        new_state_root: [u8; 32],
        /// SP1 Groth16 BN254 proof bytes (~260 B). The L1 verifier
        /// precompile validates this against `vk_hash` + the
        /// chain-state `aggregation_vk_hash`.
        proof_bytes: Vec<u8>,
        /// Public inputs to the SP1 proof. Fixed-offset SSZ layout
        /// (240 B) per Track G spec. Includes `prev_l2_state_root`,
        /// `new_l2_state_root`, `batch_id`, `da_commitment`,
        /// `l1_anchor_height`, `range_vk_commitment`,
        /// `prev_l1_state_root`, `l2_chain_id_hash`,
        /// `confidential_root` (Track H).
        public_inputs: Vec<u8>,
        /// SP1 verifying-key hash. Must equal the chain-state's
        /// `aggregation_vk_hash` (rotatable via
        /// `SetL2VerifyingKey`).
        vk_hash: [u8; 32],
    },
    /// Rotate the L2 verifying keys via governance. Per op-succinct's
    /// "multiBlockVKey" pattern, the L1 verifier expects:
    /// - `aggregation_vk_hash`: the exact SP1 vkey the precompile
    ///   verifies against
    /// - `range_vk_commitment`: per-batch range-program VK commitment
    ///   that the aggregation proof's public values embed
    ///
    /// Rotation lands at the next epoch boundary alongside other
    /// governance Intents. Authority Ring quorum (≥ ⌈2n/3⌉+1) must
    /// authorize the rotation via the standard governance path.
    SetL2VerifyingKey {
        /// New aggregation VK hash. Replaces the chain-state value
        /// consulted by the verifier precompile.
        new_aggregation_vk: [u8; 32],
        /// New range-program VK commitment. Validated against the
        /// embedded value in every subsequent aggregation proof's
        /// public inputs.
        new_range_commitment: [u8; 32],
    },
}

/// The execution substrate API consumed by the block executor.
///
/// Implementations must:
///
/// - Apply intents atomically: a failing intent leaves state unchanged.
/// - Produce a deterministic `state_root` that depends only on the
///   canonical state, not on insertion order or any other transient.
pub trait Substrate {
    /// Read the balance of `addr`. Returns zero for any unseen address.
    fn balance(&self, addr: &Address) -> Balance;

    /// Apply a single intent. On error, the substrate's state is
    /// guaranteed identical to before the call (atomicity).
    fn apply_intent(&mut self, intent: &Intent) -> Result<(), ExecutionError>;

    /// Compute the canonical state root.
    ///
    /// Encoding: BLAKE3 over `tag || for each (addr, balance) in
    /// ascending address order: addr (20 B) || balance (16 B BE)`.
    fn state_root(&self) -> [u8; 32];
}

/// Phase-1 in-memory substrate adapter.
///
/// Mirrors `gsx-db`'s `InMemoryBalanceStore` semantics. State is a
/// `BTreeMap<Address, Balance>`; zero balances are represented by absent
/// keys (the map and the explicit-zero balance produce identical roots).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InMemorySubstrate {
    balances: BTreeMap<Address, Balance>,
}

impl InMemorySubstrate {
    /// Construct an empty substrate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct with initial balances. Convenience for tests.
    pub fn from_balances<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (Address, Balance)>,
    {
        let mut s = Self::new();
        for (addr, bal) in entries {
            if bal > 0 {
                s.balances.insert(addr, bal);
            }
        }
        s
    }

    /// Total supply across all addresses (sum of balances).
    pub fn total_supply(&self) -> Balance {
        self.balances.values().sum()
    }

    /// Iterate `(address, balance)` pairs in canonical (ascending-
    /// address) order.
    pub fn entries(&self) -> impl Iterator<Item = (&Address, &Balance)> {
        self.balances.iter()
    }
}

impl Substrate for InMemorySubstrate {
    fn balance(&self, addr: &Address) -> Balance {
        self.balances.get(addr).copied().unwrap_or(0)
    }

    fn apply_intent(&mut self, intent: &Intent) -> Result<(), ExecutionError> {
        match intent {
            Intent::Transfer { from, to, amount } => {
                let (from, to, amount) = (*from, *to, *amount);
                if amount == 0 {
                    return Ok(());
                }
                let source_balance = self.balance(&from);
                if source_balance < amount {
                    return Err(ExecutionError::InsufficientBalance {
                        from,
                        have: source_balance,
                        need: amount,
                    });
                }
                // Self-transfer is a no-op AFTER balance check: the
                // sender must still have the funds, but the balance
                // does not change. Returning early avoids the
                // double-insert bug where `from == to` would overwrite
                // the (X - amount) update with the (X + amount) update,
                // inflating supply.
                if from == to {
                    return Ok(());
                }
                let dest_balance = self.balance(&to);
                let new_dest = dest_balance
                    .checked_add(amount)
                    .ok_or(ExecutionError::BalanceOverflow { to })?;

                // Atomic mutation only after both checks pass.
                let new_source = source_balance - amount;
                if new_source == 0 {
                    self.balances.remove(&from);
                } else {
                    self.balances.insert(from, new_source);
                }
                self.balances.insert(to, new_dest);
                Ok(())
            }
            // Governance variants (DAG-S25 Phase G) are no-ops at the
            // substrate level — they don't mutate balance state. The
            // daemon picks them up out of committed blocks and queues
            // them for atomic application at the next epoch boundary
            // (S25.3 + S25.4).
            Intent::AdmitAuthority { .. }
            | Intent::ExitAuthority { .. }
            | Intent::EjectAuthority { .. } => Ok(()),
            // Track G Phase G2.1 (#96): L2 state-root commitment +
            // verifying-key rotation. Phase 1 (variants added) — the
            // verifier-precompile body lands in G2.2 (#97). Until
            // then these are stub no-ops that accept the Intent so
            // upstream RPC + daemon dispatch can be exercised.
            //
            // **Reserved address invariant**: the production handler
            // in G2.2 will validate that the resulting state mutation
            // targets the `gsx_dag_l2_registry` reserved address
            // (BLAKE3("gsx-l2-registry-v1")[..20]) and reject any
            // Intent that would mutate balances at that address by
            // any other path. See `docs/iq/IQ-006-l2-state-root-
            // commitment-surface.md` for the full design.
            Intent::CommitL2StateRoot { .. } | Intent::SetL2VerifyingKey { .. } => Ok(()),
        }
    }

    fn state_root(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"GSX-STATE-ROOT-V1");
        for (addr, balance) in &self.balances {
            hasher.update(addr);
            hasher.update(&balance.to_be_bytes());
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(hasher.finalize().as_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(seed: u8) -> Address {
        [seed; 20]
    }

    #[test]
    fn empty_substrate_zero_balance() {
        let s = InMemorySubstrate::new();
        assert_eq!(s.balance(&addr(1)), 0);
        assert_eq!(s.total_supply(), 0);
    }

    #[test]
    fn transfer_atomic_on_insufficient_balance() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 50)]);
        let before_root = s.state_root();
        let err = s.apply_intent(&Intent::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 100,
        });
        assert!(matches!(
            err,
            Err(ExecutionError::InsufficientBalance { .. })
        ));
        assert_eq!(s.state_root(), before_root, "state changed despite error");
    }

    #[test]
    fn transfer_drains_source_to_zero() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 50)]);
        s.apply_intent(&Intent::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 50,
        })
        .unwrap();
        assert_eq!(s.balance(&addr(1)), 0);
        assert_eq!(s.balance(&addr(2)), 50);
    }

    #[test]
    fn transfer_zero_is_noop() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 50)]);
        let before = s.state_root();
        s.apply_intent(&Intent::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 0,
        })
        .unwrap();
        assert_eq!(s.state_root(), before);
    }

    #[test]
    fn state_root_independent_of_insertion_order() {
        let s1 = InMemorySubstrate::from_balances([(addr(1), 10), (addr(2), 20)]);
        let s2 = InMemorySubstrate::from_balances([(addr(2), 20), (addr(1), 10)]);
        assert_eq!(s1.state_root(), s2.state_root());
    }

    #[test]
    fn overflow_rejected_atomically() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 1), (addr(2), Balance::MAX)]);
        let before = s.state_root();
        let err = s.apply_intent(&Intent::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 1,
        });
        assert!(matches!(err, Err(ExecutionError::BalanceOverflow { .. })));
        assert_eq!(s.state_root(), before);
    }

    /// G2.1 stub: CommitL2StateRoot accepted; state unchanged until
    /// G2.2 wires the verifier precompile + reserved registry account.
    #[test]
    fn commit_l2_state_root_stub_is_accepted() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let before = s.state_root();
        let intent = Intent::CommitL2StateRoot {
            batch_id: 0,
            new_state_root: [0xab; 32],
            proof_bytes: vec![0xcd; 260],
            public_inputs: vec![0xef; 240],
            vk_hash: [0x42; 32],
        };
        assert!(s.apply_intent(&intent).is_ok());
        assert_eq!(
            s.state_root(),
            before,
            "G2.1 stub MUST NOT mutate state; G2.2 wires the registry account"
        );
    }

    /// G2.1 stub: SetL2VerifyingKey accepted; state unchanged until
    /// G2.2 wires the chain-state VK registry.
    #[test]
    fn set_l2_verifying_key_stub_is_accepted() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let before = s.state_root();
        let intent = Intent::SetL2VerifyingKey {
            new_aggregation_vk: [0x11; 32],
            new_range_commitment: [0x22; 32],
        };
        assert!(s.apply_intent(&intent).is_ok());
        assert_eq!(
            s.state_root(),
            before,
            "G2.1 stub MUST NOT mutate state; G2.2 wires the chain-state VK registry"
        );
    }
}
