# IQ-008 — `L2BurnProven` merkle inclusion scheme

**Status:** Ratified.
**Owner:** L2 / bridge / execution.
**Date:** 2026-05-28.
**Tracking:** Track G G3.2 phase 2 follow-up to [IQ-006](IQ-006-l2-state-root-commitment-surface.md).

## Question

`Intent::CommitL2StateRoot` writes per-batch state roots into the
reserved L2 registry account (per IQ-006), and the verifier-precompile
(`suwappu-l2-verifier-precompile::verify_l2_batch`) confirms the SP1
Groth16 BN254 proof binds them to a pinned VK before persistence. The
substrate-side accounting then trusts the state root.

`Intent::L2BurnProven` is the L2→L1 withdrawal arm: a caller asserts
"L2 batch B committed a burn of `amount` to `recipient`, paid from L2
balance to escrow." The substrate must verify the burn against the
committed L2 state root before releasing the L1 bridge escrow.

**On `main` today** the apply arm at
`crates/suwappu-execution/src/substrate.rs:2497` does THREE checks: the
batch is committed, the asset is active, and the `burn_id` (a hash
that includes `merkle_path` bytes) isn't already in the nullifier set.
It does **NOT** verify the `merkle_path` proves anything about the
committed L2 state root. The wire-format gates in `suwappu-l2-bridge`
(`merkle_path` non-empty, `% 32 == 0`, `≤ MAX_MERKLE_PATH_BYTES`) are
byte-shape only — a fabricated 32-byte string clears every gate.

A caller who knows any committed `batch_id` can construct unlimited
`(recipient, amount, fake_merkle_path)` triples — each one has a unique
`burn_id` (so the nullifier doesn't dedup) and drains escrow up to its
full balance. The whole L1↔L2 bridge is unsound until merkle inclusion
is verified.

This IQ ratifies the verification scheme so the substrate gate can
land in one PR.

## Decision

**Verify the burn against `L2StateRootRecord.state_root` via a binary
merkle path with explicit sibling-direction bits.** Concretely:

### Leaf

```text
leaf_hash = BLAKE3(
    "suwappu-l2-burn-leaf-v1" ||
    l2_chain_id_hash (32) ||
    u64_BE(batch_id) ||
    recipient (20) ||
    u128_BE(amount) ||
    u8(asset_id.is_some()) ||
    asset_id (32 if present)
)
```

Every disambiguating field of the burn participates in the leaf:
chain id, batch id, recipient, amount, asset selector. Two different
burns produce two different leaves. The domain tag (`"suwappu-l2-burn-leaf-v1"`,
18 bytes) is fixed-length so a future variant (`-v2`) is
length-distinguished, not just byte-distinguished.

`asset_id.is_some()` is a 1-byte flag and `asset_id` is included only
when present. This avoids encoding a zero-asset-id as
equivalent-to-no-asset.

### Inner node

```text
parent = BLAKE3("suwappu-l2-burn-node-v1" || left (32) || right (32))
```

Symmetric, length-distinguished domain tag. Identical to the burn-leaf
scheme except for the tag, so a leaf hash CANNOT be misread as an
inner node and vice versa.

### Path encoding

`Intent::L2BurnProven` already carries `merkle_path: Vec<u8>`
(multiple of 32 bytes per the byte-shape gate in `suwappu-l2-bridge`).
**Add a new sibling-side `Vec<u8>` field, `path_directions`**, packing
direction bits LSB-first into bytes. Path level `i` consults bit `i`
of `path_directions`: 0 = sibling is on the RIGHT of the running
hash (we are the LEFT child), 1 = sibling is on the LEFT.

The `merkle_path` field stays the existing wire shape (length is a
multiple of 32 bytes; one 32-byte chunk per level, ordered from leaf
upward). `path_directions` is a separate field, length = `ceil(levels
/ 8)` where `levels = merkle_path.len() / 32`. Out-of-range padding
bits MUST be zero — the verifier rejects non-zero padding to avoid
malleability.

Adding `path_directions` is wire-additive on the bincode-positional
`Intent` enum. Per [IQ-007](IQ-007-intent-discriminant-stability.md),
pre-mainnet variant + field churn is ratified; `#[serde(default)]` on
the new field means consumers that don't supply it default to an
empty `Vec<u8>`, which fails verification deterministically.

### Tree shape

- Bounded height: 32 levels. Already implied by
  `MAX_MERKLE_PATH_BYTES = 4096` in `suwappu-l2-bridge` (4096 / 32 = 128
  levels max, capped at 32 here for sanity).
- Sparse vs dense: not specified by this IQ. The verifier doesn't care
  how the STM builds the tree, only that the leaf-to-root path with
  the declared directions hashes to the committed `state_root`. The
  STM-side scheme (sparse merkle tree vs dense vs append-only log)
  lands in a follow-up alongside the `BatchTransaction::Burn` variant.

### Verification rule

```text
running = leaf_hash
for i in 0 .. levels:
    sibling = merkle_path[32*i .. 32*i + 32]
    if bit(path_directions, i) == 0:
        running = inner_node(running, sibling)
    else:
        running = inner_node(sibling, running)
assert running == state_root
```

`state_root` is read from the L2 registry via the existing
`(l2_chain_id_hash, batch_id)` key (already populated by
`Intent::CommitL2StateRoot`).

## Why not the alternatives

### Fold direction bits into the leading byte of each 32-byte sibling

Would keep the wire shape (no new field) but loses the property that
`merkle_path.len() % 32 == 0` — the leading direction byte would
break the per-level alignment that downstream tools (bridge UI,
explorer indexer) already depend on. Adding a separate
`path_directions` field is cheaper than reshaping `merkle_path`.

### Use the SHA3-256 family for parity with EVM

The workspace's hash primitive everywhere else (state-tree commitment
per IQ-6, burn-nullifier domain hashes, anchor `(LtpAnchorRegistry`
parity tests for ECDSA via `sha3` are a separate surface) is BLAKE3.
Mixing BLAKE3 for state and SHA3 here is asymmetric for no benefit;
the L2 STM uses BLAKE3 too (`suwappu-l2-stm::compute_state_root`).

### Verify against a `withdrawals_root` extracted from the public
inputs rather than the state root

Adds a 32-byte public-input field, shifting the 240-byte verifier
layout. Pre-mainnet this is doable (no real SP1 proofs in production
yet), but the simpler scheme — the burn leaf is a leaf in the L2
state tree, and `state_root` is the commitment — composes naturally
with the existing STM and avoids the verifier-precompile reshape. The
STM is free to structure the tree however it wants (sparse merkle
with `(chain_id_hash, batch_id, burn_index)` keys, or a dedicated
withdrawals subtree whose root is then committed into `state_root`).

## Cutover criterion

Once the L2 STM (`crates/suwappu-l2-stm`) grows a `BatchTransaction::Burn`
variant + commits burns into `new_l2_state_root`, the verifier in
this IQ becomes the consensus gate. Until then, every `L2BurnProven`
fails inclusion verification (because no burn leaves are in any
committed state tree), which is exactly the safe pre-feature posture.

## Migration

- `Intent::L2BurnProven` gains `path_directions: Vec<u8>` as a new
  field with `#[serde(default)]`. Bincode-positional means callers
  using the prior shape have an empty directions vector and the
  verifier rejects (the conservative pre-feature posture).
- Off-chain tooling (`suwappu-l2-bridge::L2BurnProvenPayload`) mirrors the
  field; its byte-shape gate validates `path_directions.len() ==
  ceil(levels / 8)`.
- No L2StateRootRecord change. The existing `state_root` IS the
  verification target.

## Implementation pointers

- `crates/suwappu-l2-bridge/src/lib.rs`: new pure helper
  `verify_burn_inclusion(leaf_fields, merkle_path, path_directions,
  state_root) -> Result<(), MerkleError>`. Implements the rule above
  with BLAKE3. Unit-tested at 256 cases default + 10k at sprint close.
- `crates/suwappu-execution/src/substrate.rs:~2497`: `Intent::L2BurnProven`
  apply arm calls `verify_burn_inclusion` after the batch-commit gate
  (line 2525), before the escrow → recipient payout (line 2554). New
  `ExecutionError::L2BurnMerkleProofRejected` variant.
- Existing tests (`l2_burn_proven_*` family at substrate.rs:3959+) use
  hand-rolled single-leaf trees: leaf with the right `(recipient,
  amount, asset_id, batch_id, chain_id_hash)`, root pinned via
  `pin_l2_state_root_for_test_with_root`, `merkle_path = vec![]` +
  `path_directions = vec![]` for a depth-0 tree.
