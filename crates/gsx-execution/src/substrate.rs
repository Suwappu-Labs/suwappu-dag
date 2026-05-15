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
}
