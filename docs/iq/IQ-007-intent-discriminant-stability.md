# IQ-007 — `Intent` enum discriminant stability (pre-mainnet vs post-cutover)

**Status:** Ratified 2026-05-25.
**Owner:** execution / node.
**Date:** 2026-05-25.
**Tracking:** issue #225. Surfaced by Codex review on PR #222
(`crates/suwappu-execution/src/substrate.rs:191`) and PR #208
(`crates/suwappu-execution/src/substrate.rs:500`).

## Question

[IQ-005](IQ-005-bincode-2x-migration.md) ratified the bincode **codec**
choice (`config::legacy()`, fixint, little-endian) and added a
1-byte frame-version marker (`FRAME_VERSION_V1 = 0x01`). That work
preserved hash-stability and wire-byte-identity across the bincode
1.x → 2.x flip, BUT it left a separate axis open: the
**discriminant ordering of `Intent` enum variants**.

bincode's enum representation is positional — variant 0 takes
discriminant byte `0x00`, variant 1 takes `0x01`, and so on, regardless
of variant name. Inserting a new variant in the middle of `Intent`
shifts every following variant's discriminant by one. Since:

- `rpc_adapter` decodes submitted `intent_bincode` payloads,
- mempool dedup hashes `blake3(bincode(intent))`,
- the consensus tx-hash recipe is the same `blake3(bincode(intent))`,
- intent-signing digests include the encoded bytes,

a variant insert is a **wire-format break**: existing hashes change,
existing signatures fail to verify, mixed-version nodes disagree on
which variant a payload represents.

Practice on `main` has been to insert new variants in semantic
positions (e.g. PR #213 `GenesisAllocation`, #214 `MintInflation`,
#215 `DistributeRewards`, #216 `Delegate`, and #222 `Undelegate*`
all added variants in the middle of the enum). Codex's static review
flags each insertion as a P1 wire-compat break. The question:

**Are these inserts acceptable today, and what's the criterion that
ends that allowance?**

## Decision

**Ratify pre-mainnet variant-insert churn.** Until the cutover
criterion below fires, contributors may insert new `Intent` variants
in semantic positions, accepting that every such insert shifts
downstream hash recipes and breaks wire compatibility for unshipped
older builds.

**No public network operating today pins a release.** The devnet
regenerates its substrate from genesis on every protocol-touching
PR; the public testnet bring-up (#223) ships behind an off-chain
operator program. The first cohort of external SDK consumers will
pin a tagged suwappu-dag release; that pin is the boundary across which
discriminants must stabilize.

### Cutover criterion

The first of the following events ends the churn-allowed regime:

1. **First public release candidate.** When the foundation publishes
   a `v0.x.0-rc1` (or `v1.0.0-rc1`) intended to be the binary a
   third party signs against.
2. **First external SDK pin.** When `clients/rust-sdk` or
   `clients/ts-sdk` ships a tagged release that downstream
   integrators (wallets, relayers, points-program operators) pin
   in their dependency manifests.
3. **Genesis-ceremony scripting begins.** Once a
   `scripts/mainnet/genesis-ceremony.sh` or equivalent lands and
   any operator's mainnet pubkey is committed to a manifest stored
   in `suwappu-papers` or another long-lived location.

At cutover, the discriminant axis becomes append-only and the
churn-allowed allowance is retired.

### Post-cutover rules

Once the cutover fires, `Intent` evolves via one of two patterns:

1. **Append at the end of the enum.** New variants land after the
   last seated one. Existing variants' discriminants do not shift.
2. **Versioned-variant pattern.** When the SHAPE of an existing
   variant must change (e.g. adding a field), introduce a new variant
   `Intent::FooV2 { ...new shape... }` alongside the existing
   `Intent::Foo { ...old shape... }`. Both variants route to the
   same internal handler at the apply boundary; old callers continue
   to encode `Foo`, new callers encode `FooV2`. Plan a deprecation
   window before retiring `Foo`.

`#[serde(default)]` on a new field of an existing variant **does
not** preserve bincode wire-compat — bincode is positional, not
tag-based — and any such PR description that claims otherwise is
incorrect. (Codex caught this on PR #208's `Intent::PostL2DA`
expansion: the PR body said `#[serde(default)]` provided wire compat;
it doesn't.)

### Enforcement (post-cutover, not landed yet)

A CI lint will check that any change to the `Intent` enum either:

- (a) only appends at the end of the enum (no shift of existing
  discriminants), OR
- (b) introduces a new versioned variant alongside the existing one
  without modifying the existing variant's field list.

The lint reads `crates/suwappu-execution/src/substrate.rs` at the
`pub enum Intent` block and compares the variant order to a checked-in
manifest pinned at cutover. The pinned manifest lives at
`docs/architecture/intent-discriminant-manifest.txt` and is updated
only by the CI bot under maintainer review.

Tracking the lint as a follow-up; do not land it pre-cutover (it
would block #222 and #208's intended fixes).

## Implications for currently-open work

- **PR #222** (`execution/undelegate-intent-v2`) — May insert
  `UndelegateBegin` and `UndelegateClaim` in their natural semantic
  position alongside `Delegate`. The Codex P1 finding cites this IQ;
  no rework needed. The other P1 on #222 (unbonding-registry
  `checked_mul`) was already addressed in 5f38f5a.
- **PR #208** (`execution/da-anchor-registry`) — The `serde(default)`
  wire-compat claim is technically wrong (see above). The intended
  redo against fresh main (per #231's locked decision) should use a
  versioned variant pattern: keep the existing 2-field
  `Intent::PostL2DA { da_blob, batch_id }` AND add the new 3-field
  `Intent::PostL2DAv2 { da_blob, batch_id, l2_chain_id_hash }` so
  old callers continue to work. Both arms route through the same
  handler internally.

## Constraints honored

- **Theorem 2 (joint quorum safety)** unaffected — the joint quorum
  reads validator-set state, not Intent shape.
- **LTP commitment surface** unaffected — LTP attestations commit a
  constant-size frame independent of `Intent`.
- **Hash recipe** stays `blake3(bincode(intent))` per IQ-005. This
  IQ governs WHICH `Intent` discriminants are stable, not HOW intents
  are hashed.

## Out of scope

- Mainnet wire format. Post-cutover, this IQ's discriminant pattern
  is locked but the wire format itself remains under IQ-005's frame
  versioning.
- Other enum surfaces (`AuthorityStatus`, `ValidatorStatus`,
  `SlashReason`, etc.). Those have their own evolution patterns
  documented at their declaration sites; this IQ scopes only to
  `Intent`.
- Migration strategy for downstream consumers post-cutover. That
  belongs in a release-engineering IQ (TBD) when the cutover
  approaches.
