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

use crate::{error::ExecutionError, reserved};

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
    /// L1→L2 deposit. User locks `amount` on L1; the sequencer
    /// reads from the live L1 event stream and credits the L2
    /// balance to `l2_recipient`. Track G Phase G3 (#90) sub-issue
    /// G3.2 (#101). Bridge accounting invariant: the L1 escrow
    /// balance equals the sum of unwithdrawn L2 deposits at every
    /// block boundary.
    L1Lock {
        /// L1 address whose balance is being locked.
        user_address: Address,
        /// L2 address to credit (may differ from `user_address`).
        l2_recipient: Address,
        /// Amount being locked (and credited on L2).
        amount: Balance,
    },
    /// L2→L1 withdrawal. Only valid AFTER a `CommitL2StateRoot` for
    /// `batch_id` has been accepted. Verifies the user's burn
    /// against the proven L2 state root via the Merkle proof. The
    /// L1 escrow then releases `amount` to `recipient`.
    L2BurnProven {
        /// L2 batch id whose committed state root proves the burn.
        batch_id: u64,
        /// L1 address receiving the unlocked balance.
        recipient: Address,
        /// Amount being unlocked.
        amount: Balance,
        /// Merkle proof binding the burn to the proven L2 state.
        merkle_path: Vec<u8>,
    },
    /// L1-quorum-enforced force-inclusion. The L2 sequencer is
    /// mandated to include `tx` in an L2 batch by
    /// `deadline_l1_height`; failure triggers `SlashSequencer` (see
    /// below) and, after `+10,000 blocks` past the deadline,
    /// permissionless `SequencerEjection`. Replay defense uses three
    /// layers: the L1 dedup hash over `(tx, deadline_l1_height,
    /// submitter)`, the `l2_nonce` enforced by the STM, and the
    /// deadline-expiry auto-eviction. See Track G #103 for the full
    /// Taiko-informed mechanics + sequencer bonding details.
    L2ForceInclude {
        /// L2-side signed transaction bytes (max 256 KB).
        tx: Vec<u8>,
        /// Hard L1 block-height deadline after which the snitch can
        /// post `Intent::SlashSequencer`.
        deadline_l1_height: u64,
        /// Submitter address (snitch-reward target if the deadline
        /// is missed).
        submitter: Address,
        /// Belt-and-suspenders L2-nonce dedup.
        l2_nonce: u64,
    },
    /// Slash a sequencer (or, post-v1.1 generalization, any
    /// slashable actor) for a verified offense. The substrate
    /// resolves the slashing-distribution waterfall per
    /// `docs/validator-sla-slashing.md` §4 + Tokenomics §8.3
    /// (counterparties → insurance pool → treasury). The actual
    /// counterparty/insurance/treasury account derivations land
    /// in the C.8 wiring PR (#131).
    SlashSequencer {
        /// Reason classification.
        reason: SlashReason,
        /// Blake3 hash referencing the offense (e.g., the missed-
        /// force-include intent hash, the equivocation-proof hash,
        /// or the invalid-batch CommitL2StateRoot hash).
        intent_hash: [u8; 32],
    },
    /// Post a per-batch DA blob to L1 calldata. The sequencer
    /// emits this alongside `CommitL2StateRoot` so the L2 state
    /// transitions are reproducible from L1-anchored data. The
    /// blob shape is documented in
    /// `docs/architecture/l2.md` (forthcoming under Track G G1.3).
    PostL2DA {
        /// Monotonic per-L2-chain batch identifier; matches
        /// `CommitL2StateRoot::batch_id`.
        batch_id: u64,
        /// Opaque DA blob bytes. Compressed L2 tx data + note
        /// commitments + nullifiers + encrypted memos.
        da_blob: Vec<u8>,
    },
    /// Distribute slashed funds per the Tokenomics §8.3 waterfall:
    ///
    /// 1. Reimburse `counterparties` (each `(addr, share)` pair).
    /// 2. Credit the insurance pool (`reserved::insurance_pool_address()`)
    ///    with `insurance_share`.
    /// 3. Credit the protocol treasury (`reserved::treasury_address()`)
    ///    with `treasury_share`.
    ///
    /// The substrate validates that **all three shares sum to the
    /// slashed amount** (carried implicitly via the sum-of-shares
    /// invariant — the substrate trusts the upstream slashing
    /// adjudicator on the per-share allocation). Crediting goes
    /// directly to balance slots, bypassing the reserved-address
    /// transfer gate (which blocks user Intents).
    ///
    /// `slash_event_id` references the upstream `SlashSequencer`
    /// (or v1.1+ `SlashAuthority`/`SlashValidator`) Intent whose
    /// `intent_hash` is the proof of cause. Adjudication +
    /// counterparty identification logic lives in the daemon's
    /// post-commit pipeline; this Intent is the deterministic
    /// substrate-level effect.
    ///
    /// Track C C.8 (#131). Mirrors the design ratified in
    /// `docs/validator-sla-slashing.md` §4.
    DistributeSlashedFunds {
        /// Reference to the originating slash event (e.g., the
        /// `SlashSequencer` Intent's blake3 content hash).
        slash_event_id: [u8; 32],
        /// Counterparties reimbursed in step 1. Empty vec is
        /// allowed (offense had no direct counterparty —
        /// equivocation, downtime — and step 1 is skipped).
        counterparties: Vec<(Address, Balance)>,
        /// Step 2 share to the insurance pool.
        insurance_share: Balance,
        /// Step 3 share to the protocol treasury.
        treasury_share: Balance,
    },
}

/// Classification of a sequencer slashing event. Drives the
/// per-class penalty + recovery path in the slashing-distribution
/// waterfall (see `docs/validator-sla-slashing.md` §3).
///
/// `#[non_exhaustive]` for the same forward-compat reasons as `Intent`
/// — additional slash classes (e.g., DA non-availability post-v1.1)
/// must be added without breaking SDK consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SlashReason {
    /// Sequencer failed to include a force-include intent by
    /// its `deadline_l1_height`. Per Track G #103: 5–10% of the
    /// liveness bond drained per occurrence, caps at 50% before
    /// full ejection.
    MissedForceInclude,
    /// Sequencer signed two conflicting L2 batches (or, by
    /// extension, an Authority node signed two conflicting fast-
    /// path certs per Paper §6.4). **100% bond forfeiture.**
    Equivocation,
    /// Submitted a `CommitL2StateRoot` whose proof verifies but
    /// whose STM execution would have violated a consensus
    /// invariant detected downstream (e.g., conservation breach
    /// in confidential transfer surface). **100% bond forfeiture.**
    InvalidBatch,
    /// Sustained downtime beyond the SLA window per
    /// `docs/validator-sla-slashing.md` §2 (medium-severity
    /// medium tier). 5% per occurrence.
    Downtime,
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

    /// Read a bytes-state record at `addr`. Returns `None` if no
    /// record is stored there. Used by reserved-address registry
    /// records (e.g., the L2 state-root registry per IQ-006 +
    /// `l2_state` module).
    ///
    /// Default impl returns `None` — implementations that don't
    /// support bytes-state (gsx-db v0.1.0) inherit the safe
    /// behavior of "no record found anywhere".
    fn read_bytes(&self, _addr: &Address) -> Option<Vec<u8>> {
        None
    }

    /// Apply a single intent. On error, the substrate's state is
    /// guaranteed identical to before the call (atomicity).
    fn apply_intent(&mut self, intent: &Intent) -> Result<(), ExecutionError>;

    /// Compute the canonical state root.
    ///
    /// Encoding (V2 — extended for bytes-state in this PR):
    /// BLAKE3 over:
    ///   `"GSX-STATE-ROOT-V2"
    ///    || balances_root
    ///    || bytes_state_root`
    /// where
    ///   balances_root    = BLAKE3("GSX-BALANCES-V1"  || foreach (addr,bal) asc: addr(20) || bal_be(16))
    ///   bytes_state_root = BLAKE3("GSX-BYTES-STATE-V1" || foreach (addr,data) asc: addr(20) || data.len() as u32 BE || data)
    ///
    /// The V1 → V2 recipe migration is a hard fork at the
    /// substrate-state-root level. No mainnet state exists yet,
    /// so this is free; testnet wipes on next re-genesis per
    /// `docs/operations/testnet-regenesis-runbook.md`.
    fn state_root(&self) -> [u8; 32];
}

/// Phase-1 in-memory substrate adapter.
///
/// Two parallel state maps:
/// - `balances`: 20-byte address → u128 balance (the existing
///   surface)
/// - `bytes_state`: 20-byte address → opaque variable-length
///   bytes (the new surface added in this PR for L2 state-root
///   registry storage + future governance/asset/registry data)
///
/// Both maps participate in the canonical `state_root` via the
/// V2 recipe documented on `Substrate::state_root`. Zero
/// balances and empty bytes-records are represented by absent
/// keys (the map and the explicit-empty/zero record produce
/// identical roots).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InMemorySubstrate {
    balances: BTreeMap<Address, Balance>,
    bytes_state: BTreeMap<Address, Vec<u8>>,
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

    /// Read the count of L2 state-root records stored at the
    /// reserved L2 registry account. Decodes the bytes-state
    /// record at `reserved::l2_registry_address()` and returns
    /// the map's size. Returns 0 if no record exists (i.e., no
    /// successful `Intent::CommitL2StateRoot` has landed yet) or
    /// if decoding fails (defensive — surfaces silently as 0
    /// rather than a panic; production callers concerned with
    /// integrity check via `l2_state_root_record` below).
    pub fn l2_commit_count(&self) -> usize {
        let bytes = self.read_bytes(&reserved::l2_registry_address());
        let map = crate::l2_state::decode_map(bytes.as_deref().unwrap_or(&[])).unwrap_or_default();
        map.len()
    }

    /// Look up a per-batch L2 state-root record by composite key.
    /// Returns `None` if no record exists or if the registry
    /// bytes are corrupt (defensive — see `l2_commit_count`).
    pub fn l2_state_root_record(
        &self,
        key: &crate::l2_state::L2BatchKey,
    ) -> Option<crate::l2_state::L2StateRootRecord> {
        let bytes = self.read_bytes(&reserved::l2_registry_address())?;
        let map = crate::l2_state::decode_map(&bytes).ok()?;
        map.get(key).copied()
    }

    /// Credit `amount` to `addr` without going through the
    /// reserved-address transfer gate. Used by the substrate-
    /// internal `DistributeSlashedFunds` arm (C.8) to deposit into
    /// the insurance pool / treasury / counterparty balance slots,
    /// and reserved for analogous protocol-owned credits in the
    /// future. Zero credits are no-ops.
    fn credit_unchecked(&mut self, addr: Address, amount: Balance) -> Result<(), ExecutionError> {
        if amount == 0 {
            return Ok(());
        }
        let new_balance = self
            .balance(&addr)
            .checked_add(amount)
            .ok_or(ExecutionError::DistributionOverflow { to: addr })?;
        self.balances.insert(addr, new_balance);
        Ok(())
    }

    /// Debit `amount` from `addr` without going through the
    /// reserved-address transfer gate. Used by the substrate-
    /// internal bridge arms (G3.2) to drain user balances into
    /// the escrow on `L1Lock` and drain the escrow into the
    /// recipient on `L2BurnProven`. Returns
    /// `InsufficientBalance` if the source can't cover.
    /// Zero debits are no-ops.
    fn debit_unchecked(&mut self, addr: Address, amount: Balance) -> Result<(), ExecutionError> {
        if amount == 0 {
            return Ok(());
        }
        let source = self.balance(&addr);
        if source < amount {
            return Err(ExecutionError::InsufficientBalance {
                from: addr,
                have: source,
                need: amount,
            });
        }
        let new_balance = source - amount;
        if new_balance == 0 {
            self.balances.remove(&addr);
        } else {
            self.balances.insert(addr, new_balance);
        }
        Ok(())
    }

    /// Read the bridge escrow's current balance. By the bridge
    /// accounting invariant, this equals the sum of unwithdrawn
    /// L2 deposits at every block boundary.
    pub fn bridge_escrow_balance(&self) -> Balance {
        self.balance(&reserved::bridge_escrow_address())
    }

    /// Internal: write a bytes-state record at `addr`. Replaces
    /// any prior record. Empty bytes are stored as absent (matches
    /// the balance map's zero-is-absent invariant + keeps the
    /// state_root canonical).
    fn write_bytes_unchecked(&mut self, addr: Address, bytes: Vec<u8>) {
        if bytes.is_empty() {
            self.bytes_state.remove(&addr);
        } else {
            self.bytes_state.insert(addr, bytes);
        }
    }
}

impl Substrate for InMemorySubstrate {
    fn balance(&self, addr: &Address) -> Balance {
        self.balances.get(addr).copied().unwrap_or(0)
    }

    fn read_bytes(&self, addr: &Address) -> Option<Vec<u8>> {
        self.bytes_state.get(addr).cloned()
    }

    fn apply_intent(&mut self, intent: &Intent) -> Result<(), ExecutionError> {
        match intent {
            Intent::Transfer { from, to, amount } => {
                let (from, to, amount) = (*from, *to, *amount);
                // C.8 reserved-address invariant: user `Transfer`
                // Intents may NOT mutate a reserved registry
                // account. Only the dedicated substrate arms (the
                // L2 verifier-precompile arm in G2.2, the
                // DistributeSlashedFunds arm below) may write to
                // those addresses.
                if reserved::is_reserved(&from) {
                    return Err(ExecutionError::ReservedAddressTransferDenied { addr: from });
                }
                if reserved::is_reserved(&to) {
                    return Err(ExecutionError::ReservedAddressTransferDenied { addr: to });
                }
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
            // Track G Phase G2.2 (#97): wired through the
            // gsx-l2-verifier-precompile crate. The verifier
            // runs format gates (proof = 260 B, public_inputs =
            // 240 B, vk_hash != all-zeros); the Groth16 BN254
            // pairing check lands once sp1-verifier is added as
            // a workspace dep.
            //
            // On verify success this PR records the per-batch
            // L2 state-root record at the reserved
            // l2_registry_address. The composite key
            // `(l2_chain_id_hash, batch_id)` is decoded from the
            // public-inputs blob at the canonical offsets per
            // gsx_l2_verifier_precompile::public_inputs.
            //
            // The map shape + encoding is defined in
            // crates/gsx-execution/src/l2_state.rs.
            Intent::CommitL2StateRoot {
                batch_id,
                new_state_root,
                proof_bytes,
                public_inputs,
                vk_hash,
            } => {
                use gsx_l2_verifier_precompile::public_inputs as pi;

                use crate::l2_state::{decode_map, encode_map, L2BatchKey, L2StateRootRecord};

                // Verifier format gate.
                gsx_l2_verifier_precompile::verify_l2_batch(proof_bytes, public_inputs, vk_hash)
                    .map_err(|e| ExecutionError::L2VerifierRejected {
                        reason: e.to_string(),
                    })?;

                // Decode L1 anchor height + l2_chain_id_hash +
                // da_commitment from the public-inputs blob at
                // their canonical offsets. The verifier's format
                // gate already guaranteed the blob is 240 B.
                let l1_anchor_height = u64::from_be_bytes(
                    public_inputs[pi::L1_ANCHOR_HEIGHT_OFFSET..pi::L1_ANCHOR_HEIGHT_OFFSET + 8]
                        .try_into()
                        .expect("public_inputs is 240 B per verifier format gate"),
                );
                let mut l2_chain_id_hash = [0u8; 32];
                l2_chain_id_hash.copy_from_slice(
                    &public_inputs[pi::L2_CHAIN_ID_HASH_OFFSET..pi::L2_CHAIN_ID_HASH_OFFSET + 32],
                );
                let mut da_commitment = [0u8; 32];
                da_commitment.copy_from_slice(
                    &public_inputs[pi::DA_COMMITMENT_OFFSET..pi::DA_COMMITMENT_OFFSET + 32],
                );

                let key = L2BatchKey {
                    l2_chain_id_hash,
                    batch_id: *batch_id,
                };
                let record = L2StateRootRecord {
                    state_root: *new_state_root,
                    committed_at_l1_height: l1_anchor_height,
                    vk_hash: *vk_hash,
                    da_commitment,
                };

                // Read the existing map (empty if first commit),
                // insert the new record, encode + write back.
                // The map encoding is the load-bearing substrate
                // state — the V2 state_root recipe hashes it via
                // the bytes_state map iteration.
                let registry_addr = reserved::l2_registry_address();
                let existing_bytes = self.read_bytes(&registry_addr).unwrap_or_default();
                let mut map = decode_map(&existing_bytes)?;
                map.insert(key, record);
                let new_bytes = encode_map(&map);
                self.write_bytes_unchecked(registry_addr, new_bytes);
                Ok(())
            }
            // `SetL2VerifyingKey` rotates a chain-state value
            // (the aggregation_vk_hash + range_vk_commitment).
            // Phase 1 (this PR / G2.2) accepts the rotation
            // without storing it; the real chain-state VK
            // registry lands in the same follow-up that adds
            // the (chain_id, batch_id) → L2StateRoot map.
            Intent::SetL2VerifyingKey { .. } => Ok(()),
            // Track G Phase G3.2 (#101): bridge accounting.
            //
            // L1Lock invariant: debit `user_address` by `amount`,
            // credit the reserved bridge_escrow_address by the
            // same amount. The bridge accounting invariant
            // (balance(bridge_escrow_address) == sum of
            // unwithdrawn L2 deposits) holds by construction
            // because every L1Lock pairs (atomically) with an
            // L2BurnProven that drains the same amount.
            //
            // Off-chain code (sequencer / prover / bridge UI)
            // should additionally validate via gsx-l2-bridge
            // before submitting — but that's a UX gate, not a
            // soundness gate.
            Intent::L1Lock {
                user_address,
                l2_recipient: _,
                amount,
            } => {
                let amount = *amount;
                let user_address = *user_address;
                if amount == 0 {
                    return Ok(());
                }
                // Atomic: balance check first via debit_unchecked
                // (returns InsufficientBalance on underrun), then
                // credit the escrow. If the credit overflows (only
                // possible at u128::MAX of unwithdrawn deposits —
                // impossible under realistic supply caps), the
                // debit is rolled back implicitly via Rust's
                // early-return + the previous-state read from
                // balance().
                self.debit_unchecked(user_address, amount)?;
                self.credit_unchecked(reserved::bridge_escrow_address(), amount)?;
                Ok(())
            }
            // L2BurnProven: drain escrow into recipient. The
            // merkle_path validation (proving the L2 burn against
            // the committed L2 state root) is a stub until G2.2
            // phase 2 stores `(chain_id, batch_id) -> L2StateRoot`.
            // Until then, the substrate enforces only the
            // accounting invariant; off-chain validators check
            // the merkle_path's byte-shape via gsx-l2-bridge.
            Intent::L2BurnProven {
                batch_id: _,
                recipient,
                amount,
                merkle_path: _,
            } => {
                let amount = *amount;
                let recipient = *recipient;
                if amount == 0 {
                    return Ok(());
                }
                if reserved::is_reserved(&recipient) {
                    return Err(ExecutionError::ReservedAddressTransferDenied { addr: recipient });
                }
                self.debit_unchecked(reserved::bridge_escrow_address(), amount)?;
                self.credit_unchecked(recipient, amount)?;
                Ok(())
            }
            // Track G G3.1 stub-arm variants still pending wiring:
            // - L2ForceInclude → G3.4 (#103) slashing integration
            // - SlashSequencer → C.8 (#131) waterfall already
            //   wired below as DistributeSlashedFunds; the
            //   SlashSequencer arm fires daemon-side adjudication
            //   that produces DistributeSlashedFunds
            // - PostL2DA → G3.3 (#102) DA blob anchoring
            Intent::L2ForceInclude { .. }
            | Intent::SlashSequencer { .. }
            | Intent::PostL2DA { .. } => Ok(()),
            // C.8 (#131): slashing-distribution waterfall.
            // Tokenomics §8.3 ordering: counterparties → insurance
            // pool → treasury. Credits go directly to balance
            // slots, bypassing the reserved-address transfer gate.
            Intent::DistributeSlashedFunds {
                slash_event_id: _,
                counterparties,
                insurance_share,
                treasury_share,
            } => {
                // Reject any counterparty pointing at a reserved
                // address — counterparty reimbursement must not be
                // redirected into the insurance/treasury (those
                // have their own dedicated shares in this same
                // Intent).
                for (addr, _) in counterparties.iter() {
                    if reserved::is_reserved(addr) {
                        return Err(ExecutionError::ReservedAddressInCounterparties {
                            addr: *addr,
                        });
                    }
                }
                // Step 1: reimburse counterparties.
                for (addr, share) in counterparties.iter() {
                    self.credit_unchecked(*addr, *share)?;
                }
                // Step 2: insurance pool.
                self.credit_unchecked(reserved::insurance_pool_address(), *insurance_share)?;
                // Step 3: protocol treasury.
                self.credit_unchecked(reserved::treasury_address(), *treasury_share)?;
                Ok(())
            }
        }
    }

    fn state_root(&self) -> [u8; 32] {
        // V2 recipe: hash the balances + bytes-state with
        // domain-separated child hashes, then combine. See the
        // Substrate trait state_root doc for the byte-by-byte
        // recipe.

        // Child 1: balances root.
        let balances_root = {
            let mut h = blake3::Hasher::new();
            h.update(b"GSX-BALANCES-V1");
            for (addr, balance) in &self.balances {
                h.update(addr);
                h.update(&balance.to_be_bytes());
            }
            let mut out = [0u8; 32];
            out.copy_from_slice(h.finalize().as_bytes());
            out
        };

        // Child 2: bytes-state root. Length-prefix each record
        // so the encoding is unambiguous across the variable-
        // length records (`u32::BE(len) || data` matches the
        // sha3_256_domain length-prefix pattern from
        // gsx-crypto::hash).
        let bytes_state_root = {
            let mut h = blake3::Hasher::new();
            h.update(b"GSX-BYTES-STATE-V1");
            for (addr, data) in &self.bytes_state {
                h.update(addr);
                h.update(&(data.len() as u32).to_be_bytes());
                h.update(data);
            }
            let mut out = [0u8; 32];
            out.copy_from_slice(h.finalize().as_bytes());
            out
        };

        // Top-level combination.
        let mut h = blake3::Hasher::new();
        h.update(b"GSX-STATE-ROOT-V2");
        h.update(&balances_root);
        h.update(&bytes_state_root);
        let mut out = [0u8; 32];
        out.copy_from_slice(h.finalize().as_bytes());
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

    /// G2.2 #97: valid CommitL2StateRoot increments the L2
    /// commit counter at the reserved registry address. The
    /// verifier format gates (proof = 260 B, public_inputs =
    /// 240 B, vk_hash != all-zeros) all pass.
    #[test]
    fn commit_l2_state_root_increments_counter() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let before_count = s.l2_commit_count();
        let intent = Intent::CommitL2StateRoot {
            batch_id: 0,
            new_state_root: [0xab; 32],
            proof_bytes: vec![0xcd; 260],
            public_inputs: vec![0xef; 240],
            vk_hash: [0x42; 32],
        };
        assert!(s.apply_intent(&intent).is_ok());
        assert_eq!(s.l2_commit_count(), before_count + 1);
    }

    /// Verifier rejects under-sized proof bytes → state unchanged.
    #[test]
    fn commit_rejects_short_proof() {
        let mut s = InMemorySubstrate::new();
        let intent = Intent::CommitL2StateRoot {
            batch_id: 0,
            new_state_root: [0xab; 32],
            proof_bytes: vec![0xcd; 259], // one byte short
            public_inputs: vec![0xef; 240],
            vk_hash: [0x42; 32],
        };
        assert!(matches!(
            s.apply_intent(&intent),
            Err(ExecutionError::L2VerifierRejected { .. })
        ));
        assert_eq!(s.l2_commit_count(), 0, "counter must not move on reject");
    }

    /// Verifier rejects wrong public-inputs width → state unchanged.
    #[test]
    fn commit_rejects_wrong_public_inputs_length() {
        let mut s = InMemorySubstrate::new();
        let intent = Intent::CommitL2StateRoot {
            batch_id: 0,
            new_state_root: [0xab; 32],
            proof_bytes: vec![0xcd; 260],
            public_inputs: vec![0xef; 100], // way too short
            vk_hash: [0x42; 32],
        };
        assert!(matches!(
            s.apply_intent(&intent),
            Err(ExecutionError::L2VerifierRejected { .. })
        ));
        assert_eq!(s.l2_commit_count(), 0);
    }

    /// Verifier rejects all-zeros vk_hash (no VK pinned in chain
    /// state) → state unchanged.
    #[test]
    fn commit_rejects_unpinned_vk() {
        let mut s = InMemorySubstrate::new();
        let intent = Intent::CommitL2StateRoot {
            batch_id: 0,
            new_state_root: [0xab; 32],
            proof_bytes: vec![0xcd; 260],
            public_inputs: vec![0xef; 240],
            vk_hash: [0u8; 32], // all-zeros = unpinned
        };
        assert!(matches!(
            s.apply_intent(&intent),
            Err(ExecutionError::L2VerifierRejected { .. })
        ));
        assert_eq!(s.l2_commit_count(), 0);
    }

    /// Repeated successful commits monotonically increment the
    /// commit counter.
    #[test]
    fn commits_are_monotonic() {
        let mut s = InMemorySubstrate::new();
        for batch_id in 0..5 {
            s.apply_intent(&Intent::CommitL2StateRoot {
                batch_id,
                new_state_root: [batch_id as u8; 32],
                proof_bytes: vec![0xcd; 260],
                public_inputs: vec![0xef; 240],
                vk_hash: [0x42; 32],
            })
            .unwrap();
        }
        assert_eq!(s.l2_commit_count(), 5);
    }

    /// Heavy: CommitL2StateRoot stores the per-batch L2 state-
    /// root record at the reserved L2 registry account, keyed by
    /// (l2_chain_id_hash, batch_id). The record carries the
    /// state_root + l1_anchor_height + vk_hash + da_commitment
    /// decoded from the public-inputs blob at the verifier-
    /// precompile's canonical offsets.
    #[test]
    fn commit_l2_state_root_stores_record() {
        use gsx_l2_verifier_precompile::public_inputs as pi;

        use crate::l2_state::L2BatchKey;

        // Construct a public-inputs blob with deterministic
        // values at each canonical offset.
        let mut public_inputs = vec![0u8; 240];
        // l1_anchor_height = 12345 at offset 104.
        public_inputs[pi::L1_ANCHOR_HEIGHT_OFFSET..pi::L1_ANCHOR_HEIGHT_OFFSET + 8]
            .copy_from_slice(&12345u64.to_be_bytes());
        // da_commitment = [0xdd; 32] at offset 72.
        public_inputs[pi::DA_COMMITMENT_OFFSET..pi::DA_COMMITMENT_OFFSET + 32]
            .copy_from_slice(&[0xdd; 32]);
        // l2_chain_id_hash = [0xc1; 32] at offset 176.
        public_inputs[pi::L2_CHAIN_ID_HASH_OFFSET..pi::L2_CHAIN_ID_HASH_OFFSET + 32]
            .copy_from_slice(&[0xc1; 32]);

        let mut s = InMemorySubstrate::new();
        s.apply_intent(&Intent::CommitL2StateRoot {
            batch_id: 7,
            new_state_root: [0xab; 32],
            proof_bytes: vec![0xcd; 260],
            public_inputs: public_inputs.clone(),
            vk_hash: [0x42; 32],
        })
        .unwrap();

        let key = L2BatchKey {
            l2_chain_id_hash: [0xc1; 32],
            batch_id: 7,
        };
        let rec = s
            .l2_state_root_record(&key)
            .expect("record must exist after commit");
        assert_eq!(rec.state_root, [0xab; 32]);
        assert_eq!(rec.committed_at_l1_height, 12345);
        assert_eq!(rec.vk_hash, [0x42; 32]);
        assert_eq!(rec.da_commitment, [0xdd; 32]);
    }

    /// Multi-batch storage: each commit lands its own record;
    /// the registry map grows monotonically; the state_root
    /// shifts after each commit.
    #[test]
    fn commit_l2_state_root_multi_batch_storage() {
        use gsx_l2_verifier_precompile::public_inputs as pi;

        use crate::l2_state::L2BatchKey;

        let mut s = InMemorySubstrate::new();
        let mut prior_root = s.state_root();

        // 3 commits to the same chain, different batch_ids.
        for batch_id in 0..3 {
            let mut public_inputs = vec![0u8; 240];
            public_inputs[pi::L1_ANCHOR_HEIGHT_OFFSET..pi::L1_ANCHOR_HEIGHT_OFFSET + 8]
                .copy_from_slice(&(100u64 + batch_id).to_be_bytes());
            public_inputs[pi::L2_CHAIN_ID_HASH_OFFSET..pi::L2_CHAIN_ID_HASH_OFFSET + 32]
                .copy_from_slice(&[0xc1; 32]);

            s.apply_intent(&Intent::CommitL2StateRoot {
                batch_id,
                new_state_root: [batch_id as u8; 32],
                proof_bytes: vec![0xcd; 260],
                public_inputs,
                vk_hash: [0x42; 32],
            })
            .unwrap();

            // state_root must change after each commit.
            let new_root = s.state_root();
            assert_ne!(prior_root, new_root, "commit must shift state_root");
            prior_root = new_root;
        }
        assert_eq!(s.l2_commit_count(), 3);

        // Every record retrievable by key.
        for batch_id in 0..3 {
            let key = L2BatchKey {
                l2_chain_id_hash: [0xc1; 32],
                batch_id,
            };
            let rec = s
                .l2_state_root_record(&key)
                .expect("each commit's record is retrievable");
            assert_eq!(rec.state_root, [batch_id as u8; 32]);
            assert_eq!(rec.committed_at_l1_height, 100 + batch_id);
        }
    }

    /// Multi-chain isolation: different `l2_chain_id_hash`
    /// values produce independent records; one chain's commits
    /// don't collide with another's even at the same batch_id.
    #[test]
    fn commit_l2_state_root_multi_chain_isolation() {
        use gsx_l2_verifier_precompile::public_inputs as pi;

        use crate::l2_state::L2BatchKey;

        let mut s = InMemorySubstrate::new();

        // Chain A, batch 0.
        let mut pi_a = vec![0u8; 240];
        pi_a[pi::L2_CHAIN_ID_HASH_OFFSET..pi::L2_CHAIN_ID_HASH_OFFSET + 32]
            .copy_from_slice(&[0xa1; 32]);
        s.apply_intent(&Intent::CommitL2StateRoot {
            batch_id: 0,
            new_state_root: [0x11; 32],
            proof_bytes: vec![0xcd; 260],
            public_inputs: pi_a,
            vk_hash: [0x42; 32],
        })
        .unwrap();

        // Chain B, batch 0 — same batch_id, different chain.
        let mut pi_b = vec![0u8; 240];
        pi_b[pi::L2_CHAIN_ID_HASH_OFFSET..pi::L2_CHAIN_ID_HASH_OFFSET + 32]
            .copy_from_slice(&[0xb1; 32]);
        s.apply_intent(&Intent::CommitL2StateRoot {
            batch_id: 0,
            new_state_root: [0x22; 32],
            proof_bytes: vec![0xcd; 260],
            public_inputs: pi_b,
            vk_hash: [0x42; 32],
        })
        .unwrap();

        let rec_a = s
            .l2_state_root_record(&L2BatchKey {
                l2_chain_id_hash: [0xa1; 32],
                batch_id: 0,
            })
            .unwrap();
        let rec_b = s
            .l2_state_root_record(&L2BatchKey {
                l2_chain_id_hash: [0xb1; 32],
                batch_id: 0,
            })
            .unwrap();
        assert_eq!(rec_a.state_root, [0x11; 32]);
        assert_eq!(rec_b.state_root, [0x22; 32]);
        assert_eq!(s.l2_commit_count(), 2);
    }

    /// state_root V2 recipe: bytes_state changes must shift the
    /// state_root even when balances are identical. Locks the
    /// invariant against a future regression that hashes only
    /// the balance map.
    #[test]
    fn state_root_v2_includes_bytes_state() {
        let s1 = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let mut s2 = InMemorySubstrate::from_balances([(addr(1), 100)]);
        assert_eq!(
            s1.state_root(),
            s2.state_root(),
            "identical balances + empty bytes_state → identical root"
        );
        // Mutate s2's bytes_state via the public CommitL2StateRoot
        // path so we don't poke the private field directly.
        let mut public_inputs = vec![0u8; 240];
        // l2_chain_id_hash bytes are zero by default — that's fine.
        s2.apply_intent(&Intent::CommitL2StateRoot {
            batch_id: 0,
            new_state_root: [0xab; 32],
            proof_bytes: vec![0xcd; 260],
            public_inputs: {
                public_inputs[gsx_l2_verifier_precompile::public_inputs::L1_ANCHOR_HEIGHT_OFFSET
                    ..gsx_l2_verifier_precompile::public_inputs::L1_ANCHOR_HEIGHT_OFFSET + 8]
                    .copy_from_slice(&42u64.to_be_bytes());
                public_inputs
            },
            vk_hash: [0x42; 32],
        })
        .unwrap();
        assert_ne!(
            s1.state_root(),
            s2.state_root(),
            "bytes_state change MUST shift the state_root (V2 recipe)"
        );
    }

    /// read_bytes returns None for unseen addresses (matches
    /// the balance read's zero-for-unseen semantics).
    #[test]
    fn read_bytes_returns_none_for_unseen() {
        let s = InMemorySubstrate::new();
        assert_eq!(s.read_bytes(&addr(1)), None);
        assert_eq!(s.read_bytes(&reserved::l2_registry_address()), None);
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

    /// G3.2 (#101): L1Lock debits user + credits the bridge
    /// escrow by `amount`. The bridge accounting invariant
    /// (balance(bridge_escrow_address) == sum of unwithdrawn
    /// L2 deposits) holds.
    #[test]
    fn l1_lock_debits_user_credits_escrow() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let intent = Intent::L1Lock {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: 25,
        };
        assert!(s.apply_intent(&intent).is_ok());
        assert_eq!(s.balance(&addr(1)), 75);
        assert_eq!(s.bridge_escrow_balance(), 25);
    }

    /// L1Lock with insufficient balance is rejected atomically.
    #[test]
    fn l1_lock_insufficient_balance_atomic_reject() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 10)]);
        let before = s.state_root();
        let intent = Intent::L1Lock {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: 25,
        };
        assert!(matches!(
            s.apply_intent(&intent),
            Err(ExecutionError::InsufficientBalance { .. })
        ));
        assert_eq!(s.state_root(), before, "state must roll back on reject");
    }

    /// Zero-amount L1Lock is a no-op (matches Transfer semantics).
    #[test]
    fn l1_lock_zero_amount_is_noop() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let before = s.state_root();
        assert!(s
            .apply_intent(&Intent::L1Lock {
                user_address: addr(1),
                l2_recipient: addr(2),
                amount: 0,
            })
            .is_ok());
        assert_eq!(s.state_root(), before);
    }

    /// G3.2: L2BurnProven debits escrow + credits recipient.
    /// Round-trip: L1Lock → L2BurnProven leaves balances
    /// conserved (escrow returns to zero).
    #[test]
    fn l2_burn_proven_drains_escrow() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        s.apply_intent(&Intent::L1Lock {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: 25,
        })
        .unwrap();
        assert_eq!(s.bridge_escrow_balance(), 25);
        s.apply_intent(&Intent::L2BurnProven {
            batch_id: 7,
            recipient: addr(1),
            amount: 25,
            merkle_path: vec![0xab; 256],
        })
        .unwrap();
        assert_eq!(s.bridge_escrow_balance(), 0);
        assert_eq!(s.balance(&addr(1)), 100, "round-trip conserves balance");
    }

    /// L2BurnProven cannot drain escrow below zero.
    #[test]
    fn l2_burn_proven_insufficient_escrow_rejected() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        // Escrow has zero balance.
        let intent = Intent::L2BurnProven {
            batch_id: 7,
            recipient: addr(1),
            amount: 25,
            merkle_path: vec![0xab; 256],
        };
        assert!(matches!(
            s.apply_intent(&intent),
            Err(ExecutionError::InsufficientBalance { .. })
        ));
    }

    /// L2BurnProven cannot target a reserved address as recipient.
    #[test]
    fn l2_burn_proven_reserved_recipient_rejected() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        // First fund the escrow.
        s.apply_intent(&Intent::L1Lock {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: 25,
        })
        .unwrap();
        // Then attempt to withdraw to a reserved address.
        let intent = Intent::L2BurnProven {
            batch_id: 7,
            recipient: reserved::treasury_address(),
            amount: 25,
            merkle_path: vec![0xab; 256],
        };
        assert!(matches!(
            s.apply_intent(&intent),
            Err(ExecutionError::ReservedAddressTransferDenied { .. })
        ));
    }

    /// Multiple L1Lock + L2BurnProven preserve the bridge
    /// accounting invariant across an arbitrary sequence.
    #[test]
    fn bridge_accounting_invariant_holds_across_sequence() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 1000), (addr(2), 500)]);
        // Deposit 100 from addr(1).
        s.apply_intent(&Intent::L1Lock {
            user_address: addr(1),
            l2_recipient: addr(3),
            amount: 100,
        })
        .unwrap();
        assert_eq!(s.bridge_escrow_balance(), 100);
        // Deposit 50 from addr(2).
        s.apply_intent(&Intent::L1Lock {
            user_address: addr(2),
            l2_recipient: addr(4),
            amount: 50,
        })
        .unwrap();
        assert_eq!(s.bridge_escrow_balance(), 150);
        // Withdraw 70.
        s.apply_intent(&Intent::L2BurnProven {
            batch_id: 1,
            recipient: addr(5),
            amount: 70,
            merkle_path: vec![0xab; 32],
        })
        .unwrap();
        assert_eq!(s.bridge_escrow_balance(), 80);
        assert_eq!(s.balance(&addr(5)), 70);
        // Withdraw remaining 80.
        s.apply_intent(&Intent::L2BurnProven {
            batch_id: 2,
            recipient: addr(6),
            amount: 80,
            merkle_path: vec![0xab; 32],
        })
        .unwrap();
        assert_eq!(s.bridge_escrow_balance(), 0);
        assert_eq!(s.balance(&addr(6)), 80);
        // Conservation: sum unchanged.
        let total: Balance = s.entries().map(|(_, b)| *b).sum();
        assert_eq!(
            total, 1500,
            "total supply preserved across bridge round-trip"
        );
    }

    /// G3.1 stub: L2ForceInclude accepted; state unchanged until G3.4
    /// (#103) wires the slashing test.
    #[test]
    fn l2_force_include_stub_is_accepted() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let before = s.state_root();
        let intent = Intent::L2ForceInclude {
            tx: vec![0xde, 0xad, 0xbe, 0xef],
            deadline_l1_height: 1_234_567,
            submitter: addr(1),
            l2_nonce: 5,
        };
        assert!(s.apply_intent(&intent).is_ok());
        assert_eq!(s.state_root(), before);
    }

    /// G3.1 stub: SlashSequencer accepted; state unchanged until C.8
    /// (#131) wires the slashing-distribution waterfall.
    #[test]
    fn slash_sequencer_stub_is_accepted() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let before = s.state_root();
        for reason in [
            SlashReason::MissedForceInclude,
            SlashReason::Equivocation,
            SlashReason::InvalidBatch,
            SlashReason::Downtime,
        ] {
            let intent = Intent::SlashSequencer {
                reason,
                intent_hash: [0x42; 32],
            };
            assert!(s.apply_intent(&intent).is_ok());
            assert_eq!(
                s.state_root(),
                before,
                "G3.1 stub MUST NOT mutate state for reason {reason:?}"
            );
        }
    }

    /// G3.1 stub: PostL2DA accepted; state unchanged.
    #[test]
    fn post_l2_da_stub_is_accepted() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let before = s.state_root();
        let intent = Intent::PostL2DA {
            batch_id: 42,
            da_blob: vec![0xcd; 1024],
        };
        assert!(s.apply_intent(&intent).is_ok());
        assert_eq!(s.state_root(), before);
    }

    /// SlashReason variants are distinct + serializable.
    /// Defends against accidental aliasing in the slashing-
    /// distribution waterfall (C.8) — each reason maps to a
    /// different penalty band per the SLA doc.
    #[test]
    fn slash_reason_variants_are_distinct() {
        let variants = [
            SlashReason::MissedForceInclude,
            SlashReason::Equivocation,
            SlashReason::InvalidBatch,
            SlashReason::Downtime,
        ];
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "SlashReason variants {i} and {j} alias");
                }
            }
        }
    }

    // ----- C.8: reserved-address gate + slashing-distribution -----

    /// Transfer INTO a reserved address (insurance pool) is rejected.
    #[test]
    fn transfer_to_reserved_address_is_rejected() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let err = s.apply_intent(&Intent::Transfer {
            from: addr(1),
            to: reserved::insurance_pool_address(),
            amount: 10,
        });
        assert!(matches!(
            err,
            Err(ExecutionError::ReservedAddressTransferDenied { .. })
        ));
    }

    /// Transfer FROM a reserved address (treasury) is rejected.
    /// Symmetric with the `to` check — user Intents may not drain
    /// reserved accounts via Transfer either. The treasury is
    /// only spent by future governance Intents.
    #[test]
    fn transfer_from_reserved_address_is_rejected() {
        let mut s = InMemorySubstrate::new();
        let err = s.apply_intent(&Intent::Transfer {
            from: reserved::treasury_address(),
            to: addr(2),
            amount: 10,
        });
        assert!(matches!(
            err,
            Err(ExecutionError::ReservedAddressTransferDenied { .. })
        ));
    }

    /// Step 1 only: distribution with counterparties, zero
    /// insurance + treasury shares. All 3 waterfall steps exercised.
    #[test]
    fn distribute_slashed_funds_to_counterparties_only() {
        let mut s = InMemorySubstrate::new();
        let intent = Intent::DistributeSlashedFunds {
            slash_event_id: [0xaa; 32],
            counterparties: vec![(addr(1), 30), (addr(2), 70)],
            insurance_share: 0,
            treasury_share: 0,
        };
        s.apply_intent(&intent).unwrap();
        assert_eq!(s.balance(&addr(1)), 30);
        assert_eq!(s.balance(&addr(2)), 70);
        assert_eq!(s.balance(&reserved::insurance_pool_address()), 0);
        assert_eq!(s.balance(&reserved::treasury_address()), 0);
    }

    /// Steps 2 + 3 only: no counterparties (e.g., equivocation slash).
    #[test]
    fn distribute_slashed_funds_insurance_and_treasury() {
        let mut s = InMemorySubstrate::new();
        let intent = Intent::DistributeSlashedFunds {
            slash_event_id: [0xbb; 32],
            counterparties: vec![],
            insurance_share: 1_000_000,
            treasury_share: 500_000,
        };
        s.apply_intent(&intent).unwrap();
        assert_eq!(s.balance(&reserved::insurance_pool_address()), 1_000_000);
        assert_eq!(s.balance(&reserved::treasury_address()), 500_000);
    }

    /// All 3 steps in one distribution. Mirrors Tokenomics §8.3
    /// ordering literally.
    #[test]
    fn distribute_slashed_funds_full_waterfall() {
        let mut s = InMemorySubstrate::new();
        let intent = Intent::DistributeSlashedFunds {
            slash_event_id: [0xcc; 32],
            counterparties: vec![(addr(1), 100)],
            insurance_share: 200,
            treasury_share: 300,
        };
        s.apply_intent(&intent).unwrap();
        assert_eq!(s.balance(&addr(1)), 100);
        assert_eq!(s.balance(&reserved::insurance_pool_address()), 200);
        assert_eq!(s.balance(&reserved::treasury_address()), 300);
    }

    /// Distribution that names a reserved address as a counterparty
    /// is rejected — counterparty reimbursement may NOT be
    /// redirected into the insurance / treasury (those have their
    /// own dedicated shares in the same Intent).
    #[test]
    fn distribute_rejects_reserved_counterparty() {
        let mut s = InMemorySubstrate::new();
        let err = s.apply_intent(&Intent::DistributeSlashedFunds {
            slash_event_id: [0xdd; 32],
            counterparties: vec![(reserved::treasury_address(), 100)],
            insurance_share: 0,
            treasury_share: 0,
        });
        assert!(matches!(
            err,
            Err(ExecutionError::ReservedAddressInCounterparties { .. })
        ));
    }

    /// Zero-amount credits in any step are no-ops; do NOT create
    /// phantom zero balances that would shift the state root.
    #[test]
    fn distribute_zero_shares_are_noops() {
        let mut s = InMemorySubstrate::from_balances([(addr(9), 1)]);
        let before = s.state_root();
        s.apply_intent(&Intent::DistributeSlashedFunds {
            slash_event_id: [0xee; 32],
            counterparties: vec![(addr(1), 0)],
            insurance_share: 0,
            treasury_share: 0,
        })
        .unwrap();
        assert_eq!(
            s.state_root(),
            before,
            "zero-share distributions MUST NOT mutate state"
        );
    }

    /// Multiple distributions to the same counterparty are
    /// additive (don't overwrite the prior credit).
    #[test]
    fn distribute_is_additive() {
        let mut s = InMemorySubstrate::new();
        for _ in 0..3 {
            s.apply_intent(&Intent::DistributeSlashedFunds {
                slash_event_id: [0x01; 32],
                counterparties: vec![(addr(5), 100)],
                insurance_share: 50,
                treasury_share: 25,
            })
            .unwrap();
        }
        assert_eq!(s.balance(&addr(5)), 300);
        assert_eq!(s.balance(&reserved::insurance_pool_address()), 150);
        assert_eq!(s.balance(&reserved::treasury_address()), 75);
    }
}
