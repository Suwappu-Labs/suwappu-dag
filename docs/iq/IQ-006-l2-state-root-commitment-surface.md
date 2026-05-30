# IQ-006 — L2 state-root commitment surface

**Status:** Recommendation, pending sign-off after Phase G2
implementation work lands (issue #99).
**Owner:** L2 / execution
**Date:** 2026-05-16
**Tracking:** Track G Phase G2 verifier-precompile epic
(issue #89), sub-issue #98.

## Question

The zk-rollup L2 (Track G) needs to commit per-batch L2 state
roots onto the L1 (gsx-dag) so that:

1. The L1-side verifier precompile (`crates/gsx-l2-verifier-precompile/`,
   issue #97) can prove that a given L2 state root was attested
   to by a valid SP1 Groth16 BN254 proof, and
2. The L1 bridge (`crates/gsx-l2-bridge/`, issue #101) can resolve
   `L2BurnProven` withdrawal claims against the correct
   batch-level state root, and
3. The multi-L2 forward-compatibility case (multiple L2 chains
   per gsx-dag L1 in v1.1+) is preserved without a hard fork.

**Where on the L1 chain does the L2 state root live?** Three
options were surveyed. None map onto an existing data structure
without modification.

## Options surveyed

### Option A — Verifier-precompile registry account (RECOMMENDED)

A reserved L1 account (`gsx_dag_l2_registry`, derived as
`BLAKE3("gsx-l2-registry-v1")[..20]`) is owned by the verifier
precompile. Each successful `Intent::CommitL2StateRoot`
execution writes a `(chain_id, batch_id) → l2_state_root`
mapping into the account's state. The account is not
user-spendable; only the verifier precompile can mutate it.

**Pros:**
- Multi-L2 forward-compat: a new L2 chain is a single
  `chain_id` field add at the registry-account-key level.
  No hard fork.
- L1 checkpoint cadence (`crates/gsx-execution/src/checkpoint.rs:40-69`)
  stays independent of L2 batch cadence. Checkpoints commit
  the balance map; L2 state roots live in the substrate
  state-map alongside it, but as a separately-keyed registry.
- Matches Filecoin / op-succinct production pattern:
  chain-state values, not contract storage, not block-header
  extension.
- The L2 verifier precompile already owns the dispatch path
  (per the audit finding that precompiles are NOT a registry
  today — they're standalone validation modules with no
  `apply_intent` integration). Adding the registry account is
  the same dispatch-surface extension already required by the
  rest of Track G.

**Cons:**
- Introduces the "reserved registry account" concept the
  executor must reserve. Not yet a pattern in the workspace —
  the existing precompiles (`crates/gsx-precompiles/{did,
  did_resolver, issuer, reserve}`) are standalone modules
  without registry-account semantics.
- Subtle: the address derivation
  (`BLAKE3("gsx-l2-registry-v1")[..20]`) must be reserved against
  collision with user-derived addresses. With 160-bit address
  space and a domain tag, collision probability is
  negligible — but the address MUST be documented as reserved
  and the substrate MUST reject any `Intent::AdmitAuthority` /
  `Intent::Transfer` etc. targeting this address.

### Option B — Extend `Checkpoint` struct

Add `l2_state_roots: Vec<L2StateRoot>` to
`crates/gsx-execution/src/checkpoint.rs:40-69` (the existing
`Checkpoint { height, round, state_root, prev_checkpoint }`
struct) and update the hash recipe accordingly.

**Pros:**
- Single source of truth: the checkpoint hash binds the L2
  state roots automatically. Authority Ring 7-of-9 cosignature
  on the checkpoint covers L2 attestation by construction.
- No new registry-account concept needed.

**Cons:**
- **Couples L2 commit cadence to L1 checkpoint cadence.**
  Checkpoints fire at a configured interval; L2 batches fire
  every 5–10 seconds. Either L2 batches stall waiting for
  checkpoint cadence, OR checkpoint cadence accelerates to
  match L2 (load-bearing on L1 throughput).
- **Multi-L2 requires a schema revision** to the
  `Checkpoint` struct. Adding L2 chain #2 in v1.1 is a hard
  fork at the checkpoint-hash level.
- **Breaks every existing checkpoint consumer**: indexer,
  authority cosignature aggregator, gsx-mempool dedup logic
  that hashes checkpoints, and any DAG-S11 sprint code that
  computes `Checkpoint::hash()`.

### Option C — New top-level state field

Add `l2_roots: BTreeMap<ChainId, L2Root>` parallel to the
balance map. The executor maintains both trees; the L1 state
root is computed over both.

**Pros:**
- No checkpoint schema change.

**Cons:**
- **Two state trees to keep in sync** at every block boundary.
  The L1 state-root recipe (`crates/gsx-execution/src/substrate.rs:202-212`,
  `BLAKE3("GSX-STATE-ROOT-V1" || (addr || balance) sorted)`)
  has to be extended to a multi-tree hash, complicating audit
  and historical state queries.
- More invasive than Option A: Option A keeps the L2 state
  roots inside the existing balance-map abstraction (just at
  a reserved address), so the state-root recipe stays
  untouched.

## Decision

**Option A — Verifier-precompile registry account.**

Mirrors op-succinct's production pattern (the closest analogue
in the SP1 ecosystem). Multi-L2 forward-compat is a single
`chain_id` field add. The reserved registry account is owned
by the verifier precompile, not user-spendable.

### Address derivation

The L2 registry account address is computed deterministically:

```
L2_REGISTRY_ADDRESS = BLAKE3("gsx-l2-registry-v1")[..20]
```

This uses `sha3_256_domain`-style domain tagging via the
existing pattern at `crates/gsx-crypto/src/hash.rs` (the
`sha3_256_domain` helper, length-prefix tagged). Implementation
uses the BLAKE3 variant for consistency with the existing
state-root recipe (`crates/gsx-execution/src/substrate.rs:202-212`,
which already uses BLAKE3 with the `GSX-STATE-ROOT-V1` domain
tag).

The address MUST be reserved against collision: the substrate
MUST reject any `Intent::AdmitAuthority`, `Intent::Transfer`,
or other Intent that targets this address as the `from`,
`to`, or `authority_id`-derived owner. This is enforced in
both `InMemorySubstrate` (`substrate.rs:152-200`) and
`GsxDbSubstrate` (`substrate.rs:106-146`) impls.

### Registry account state shape

The reserved account stores a map of `(chain_id, batch_id) →
L2StateRoot`:

```rust
pub struct L2StateRoot {
    pub state_root: [u8; 32],         // L2 MPT root (per EVM convention post Open Item #8 flip)
    pub committed_at_l1_height: u64,  // L1 block height when CommitL2StateRoot was applied
    pub vk_hash: [u8; 32],            // The aggregation_vk_hash that verified the proof
    pub da_commitment: [u8; 32],      // BLAKE3(da_blob) for DA availability binding
}
```

The map is keyed by `(chain_id, batch_id)` to support multi-L2
in v1.1+. At v1 there is exactly one L2 chain (`gsx-l2-mainnet-1`
post-genesis), but the key structure is identical.

### Per-PR scope (Phase G2 implementation, issue #97)

1. **Reserve the address** in the substrate boot path. Define
   a `RESERVED_ADDRESSES: &[Address]` const in
   `crates/gsx-execution/src/substrate.rs` and reject any
   `Intent` whose effect would mutate or transfer to a
   reserved address.
2. **`Intent::CommitL2StateRoot` arm** in both Substrate
   impls writes to the registry account via a new internal
   helper `write_l2_state_root(chain_id, batch_id, root)`.
3. **`Intent::SetL2VerifyingKey` arm** in both Substrate
   impls writes the `(aggregation_vk_hash, range_vk_commitment)`
   pair to a separate sub-key of the registry account. Per
   op-succinct's "multiBlockVKey" trick, the
   `range_vk_commitment` is also embedded in the aggregation
   proof's public values; the L1 verifier checks both for
   consistency.
4. **Reader API**: `crates/gsx-rpc/src/methods.rs` exposes
   `gsx_getL2StateRoot { chain_id, batch_id } → L2StateRoot`
   for the bridge contracts (`crates/gsx-l2-bridge/`, issue
   #101) and the L2 explorer (issue #110).

### What this decision does NOT change

- The L1 state-root recipe at
  `crates/gsx-execution/src/substrate.rs:202-212` stays as
  `BLAKE3("GSX-STATE-ROOT-V1" || (addr || balance) sorted)`.
  The registry account is one address in the balance map;
  its mutation flows through the same state-root computation.
- The `Checkpoint` struct at
  `crates/gsx-execution/src/checkpoint.rs:40-69` is unchanged.
  L2 state roots are NOT in the checkpoint hash directly,
  but they ARE in the L1 state root (via the registry
  account), which IS in the checkpoint hash. Same security
  property; better separation of concerns.
- Existing precompile modules (`crates/gsx-precompiles/`)
  remain standalone validation modules. The L2 verifier
  precompile is a NEW dispatch surface, not an extension of
  the existing `precompiles` crate. This matches the
  internal repo audit finding that `apply_intent` does not
  currently dispatch to precompiles.

## L1↔L2 binding (the SHA3 trick)

Per Open Item #12 (ratified separately), the L2 STARK proof's
public-input layout includes a `prev_l1_state_root: [u8; 32]`
field. One in-circuit SHA3-256 binds the L2 MPT root (in the
public inputs) to the L1 BLAKE3 state-root commitment at
`l1_anchor_height`. This is the "one Keccak-f per batch"
pattern from the institutional zk-rollup research.

The verifier precompile validates this binding:

```rust
// Inside crates/gsx-l2-verifier-precompile/src/lib.rs (issue #97):
fn verify_l2_batch(
    proof: &[u8],
    public_inputs: &[u8],
    vk_hash: [u8; 32],
) -> Result<(), VerifyError> {
    // 1. Decode public inputs.
    let pi = PublicInputs::decode(public_inputs)?;

    // 2. Lookup the registry's stored (aggregation_vk_hash, range_vk_commitment).
    let (stored_agg_vk, stored_range_commit) = registry_lookup_vks()?;
    if vk_hash != stored_agg_vk { return Err(VkMismatch); }
    if pi.range_vk_commitment != stored_range_commit { return Err(RangeVkMismatch); }

    // 3. Verify the L1 anchor binding.
    let expected_l1_root = state_root_at_height(pi.l1_anchor_height)?;
    if pi.prev_l1_state_root != expected_l1_root { return Err(AnchorMismatch); }

    // 4. Verify the SP1 Groth16 proof itself.
    sp1_verifier::Groth16Verifier::verify_proof(
        proof,
        public_inputs,
        &hex::encode(vk_hash),
        SP1_GROTH16_VK_BYTES,
    ).map_err(VerifyError::Sp1)
}
```

## Open sub-question — address derivation primitive

The decision specifies `BLAKE3("gsx-l2-registry-v1")[..20]`.
An alternative was considered:

- **(a) Hardcoded constant `[u8; 20]`**: simpler to reason
  about, no derivation. Downside: not hash-traceable; reviewers
  can't verify the constant matches an intended domain.
- **(b) BLAKE3-derived (recommended)**: hash-traceable + collision-
  resistant + matches the existing domain-tag pattern in
  `crates/gsx-crypto/src/hash.rs`.

Recommend **(b)**. Lock the derivation in `crates/gsx-execution/src/substrate.rs`
as a `const` evaluated at compile time (via
`blake3::hash` in a `const` context if MSRV allows; otherwise
`OnceLock`).

## Constraints honored

- **Wire format**: no change to existing Intent wire format
  for non-L2 Intents. The 1-byte `FRAME_VERSION_V1` from IQ-005
  remains; new L2 Intent variants (`CommitL2StateRoot`,
  `SetL2VerifyingKey`, `L1Lock`, `L2BurnProven`,
  `L2ForceInclude`, `SlashSequencer`, `PostL2DA`) ride the
  existing wire-frame protocol via the `#[non_exhaustive]`
  Intent enum at `substrate.rs:40`.
- **Hash stability**: the existing
  `blake3(bincode(intent))` content-hash recipe at
  `crates/gsx-mempool/src/mempool.rs:146` continues to work
  for new Intent variants without modification.
- **Audit surface**: the registry-account address derivation
  + the reserved-address check are in scope for Track A.2
  (Trail of Bits consensus audit). Surfacing the "reserved
  address" pattern as a documented invariant before audit
  kickoff reduces audit findings.
- **No `--workspace` cargo commands on this Mac** per
  `GSXHELPER.md`. Per-crate `cargo check -p gsx-execution
  -p gsx-node -p gsx-rpc -p gsx-fastpath -p gsx-mempool`
  validates the `#[non_exhaustive]` propagation for new
  Intent variants. CI matrix validates the rest.

## Future cutovers

Multi-L2 in v1.1+ is a no-op at the registry level: the second
L2 chain gets its own `chain_id` (e.g., `gsx-l2-mainnet-2`)
and writes into the same registry account at a different
`(chain_id, _)` key prefix. No hard fork required.

If we ever decide to move L2 state roots into the checkpoint
struct (Option B), the migration is:

1. Add `Checkpoint::l2_state_roots: Vec<L2StateRoot>` field
   behind a feature flag.
2. Substrate code reads from the registry account but ALSO
   writes the same root into the new Checkpoint field at each
   checkpoint cadence.
3. At a flag day, switch consumers to read from the Checkpoint
   field; deprecate the registry-account read path.
4. Eventually drop the registry account in a hard fork.

This migration path is documented but not recommended for v1
or v1.1.

## See also

- [`crates/gsx-execution/src/substrate.rs:40`](../../crates/gsx-execution/src/substrate.rs) —
  the `#[non_exhaustive]` Intent enum that gains the new
  variants.
- [`crates/gsx-execution/src/substrate.rs:202-212`](../../crates/gsx-execution/src/substrate.rs) —
  the L1 state-root recipe.
- [`crates/gsx-execution/src/checkpoint.rs:40-69`](../../crates/gsx-execution/src/checkpoint.rs) —
  the `Checkpoint` struct + hash recipe.
- [`crates/gsx-crypto/src/hash.rs`](../../crates/gsx-crypto/src/hash.rs) —
  the `sha3_256_domain` length-prefix tag pattern.
- [`crates/gsx-mempool/src/mempool.rs:146`](../../crates/gsx-mempool/src/mempool.rs) —
  the content-hash recipe that new Intent variants inherit.
- [op-succinct architecture](https://succinctlabs.github.io/op-succinct/architecture.html) —
  closest production reference for VK-management + registry-
  account patterns.
- Phase G2 epic: issue #89.
- Verifier-precompile crate: issue #97.
- Bridge handlers: issue #101.
