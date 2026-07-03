# IQ-010 — Cross-chain interop: OFT/CCTP-class asset-mobility adapter vs LTP-only

**Status:** Recommendation, pending sign-off. Phased and **gated on
BRIDGE-1** (`submitHeader` → quorum oracle → mint path) landing first.
**Owner:** LTP / bridge / interop
**Date:** 2026-07-03
**Tracking:** [`docs/research/competitive-gap-analysis.md`](../research/competitive-gap-analysis.md)
gap **G-9** ("Interop is proprietary — LTP only") and workstream
**INTEROP-1** (§6, P2). Landscape evidence:
[`docs/research/briefs/landscape.md`](../research/briefs/landscape.md) §4
(cross-chain settlement standards).

## Question

**Should suwappu-dag add an OFT-class (LayerZero-style) or CCTP-class
(Circle burn-and-mint) cross-chain asset-mobility adapter, and how does
that relate to the LTP attestation layer — are they the same layer or two
layers?**

The gap analysis (G-9) flags that stablecoin interop has been won by two
standards suwappu-dag does not speak: **CCTP v2** (Circle burn-and-mint,
native USDC/EURC across 13+ chains, issuer-only) and **LayerZero OFT**
(the default for everything else — USDT0 moved **$70B+ cross-chain in its
first 12 months**, and Tether made a strategic investment in LayerZero
Labs on 2026-02-10). In practice the two are complementary and composed
by aggregators (LI.FI, Squid, deBridge). suwappu-dag has none of this.

### The central framing (the mistake to avoid)

**LTP is a settlement-attestation layer, not an asset bridge.** The
temptation is to point at the LTP corridor attestation and the
bridge-header module and say "we already have interop." We do not. Both
are *attestations* — constant-size cryptographic claims that a chain
reached a given state — and neither moves an asset:

- **`crates/suwappu-ltp/src/attestation.rs`** — the 7-of-9 super-node
  BLS12-381 aggregate signs an `AttestationPayload {source_chain,
  target_chain, source_height, state_root, timestamp_round}`
  (`attestation.rs:84-110`). That is a witnessed statement *"the source
  chain reached this `state_root` at this height."* It carries no amount,
  no recipient, no mint instruction. It is the ≈1,600 B constant-size
  commitment of Invariant 3 (`lib.rs:45-46`) — a proof of *settlement
  state*, deliberately payload-independent.
- **`crates/suwappu-consensus/src/bridge_header.rs`** — the source-side
  ML-DSA-65 validator-quorum side-attestation signs a 148-byte preimage
  over `(network_id, oracle, block_number, state_root)`
  (`bridge_header.rs:63-80`). Its own module doc is explicit: this is a
  *side-attestation*, "**NOT** trustless... nothing in this module is
  wired into the consensus loop, the daemon, or the mint path. The
  oracle/registry on the contract side is currently UNFED"
  (`bridge_header.rs:17-22`). `state_root` is the BLAKE3 L1 state root,
  "**not** an EVM-MPT root and is therefore **not** storage-provable"
  (`bridge_header.rs:117-122`).

An asset-mobility standard (OFT / CCTP) is a different layer that sits
*on top of* an attestation/settlement layer: it defines a lock/burn on
the source, a canonical message, and a mint/unlock on the destination.
LTP can be the settlement substrate that such a layer trusts, but LTP is
not itself that layer. **Conflating the two is the error G-9 is warning
against.**

### What interop actually exists today (honest inventory)

| Surface | What it does | What it is NOT |
|---|---|---|
| LTP corridor attestation (`suwappu-ltp`) | 7-of-9 BLS aggregate over an observed source-chain `state_root`; constant-size ≈1,600 B commitment | Not an asset move; no amount/recipient; BLS12-381 is **classical** (documented PQ-exception, G-8) |
| Bridge-header attestation (`suwappu-consensus/src/bridge_header.rs`) | Per-validator ML-DSA-65 signed claim over `(block_number, state_root)`; served via `suwappu_getHeaderAttestation` RPC | Oracle UNFED; not wired to consensus or a mint path; opaque anchor, not storage-provable |
| L1↔L2 bridge (`suwappu-l2-bridge`) | Payload validation for `L1Lock` (deposit) / `L2BurnProven` (withdrawal) into suwappu-dag's **own** zk-L2 escrow | Phase-1 byte-shape checks only (`lib.rs:19-31`); full Merkle inclusion pending G2.2 phase 2; **internal** L1↔own-L2, not external-chain interop |

**Net:** we have two attestation surfaces and one internal L1↔L2 escrow
path. We have **no adapter that moves a suwappu-dag-native asset to or
from an external chain** — no OFT endpoint, no CCTP domain, no burn-and-mint
message. That is exactly the G-9 gap, and the mint path that any such
adapter would need is itself **not yet wired** (BRIDGE-1, see below).

## Options surveyed

### Option A — LTP = attestation/settlement only, add a thin OFT-class adapter (RECOMMENDED)

Draw the layer boundary explicitly. LTP stays what it is: the
constant-size settlement-attestation layer (a proof that suwappu-dag
reached a state). Asset mobility for suwappu-dag-native assets rides a
**thin OFT-class adapter** that lets those assets move on LayerZero's
existing mesh rather than building and bootstrapping our own
liquidity/route graph.

Where the adapter sits relative to the quorum-header oracle: the adapter
is a *consumer* of the settlement layer, not a replacement for it. The
suwappu-dag side of an OFT send is a **burn (or lock)** recorded in L1
state; the fact that the burn was finalized is exactly what the
bridge-header ML-DSA-65 quorum attestation
(`bridge_header.rs:108-137`) already certifies to an external oracle.
The OFT adapter contract on suwappu-dag calls into (or is gated by) the
same **`SuwappuDagQuorumHeaderOracle` → mint** finalization path that
BRIDGE-1 wires — i.e., the adapter sits *above* the quorum-header oracle
and reuses it as its finality source, instead of trusting LayerZero's
DVN set for suwappu-dag-side finality. On the far side, the destination
OFT endpoint mints the peer representation via LayerZero's standard
messaging.

**Pros:**
- Rents distribution instead of building it. §4 of the landscape brief
  and §6 ("distribution beats technology") both say the same thing: the
  OFT mesh is where the volume is ($70B+ USDT0; Tether-invested). A thin
  adapter plugs into an existing route graph and aggregator coverage
  (LI.FI/Squid) for free.
- Keeps the LTP invariant clean. LTP's constant-size ≈1,600 B commitment
  (`lib.rs:45-46`, Invariant 3) is untouched — the adapter does not add
  per-payload bytes to the on-chain LTP commitment surface, because it is
  a *separate* contract path, not an extension of the attestation
  envelope.
- Reuses the PQ-attestation moat where it matters. The suwappu-dag-side
  finality the adapter depends on can be the **ML-DSA-65** quorum-header
  attestation (`bridge_header.rs`), letting us market "our leg of the
  bridge is post-quantum-finalized" — a real, defensible half.

**Cons:**
- **PQ posture tension with Invariant 2.** The OFT transport itself
  (LayerZero's DVN / executor security model) is **classical** and
  outside our trust boundary. An asset that has crossed onto the OFT
  mesh inherits LayerZero's classical security, not ML-DSA-65. This must
  be documented as an **exception zone** (like the LTP BLS12-381
  exception, G-8), with the honest framing that *only the suwappu-dag leg*
  is PQ-finalized; the cross-mesh hop is classical. Invariant 2 permits
  classical primitives only on documented exception zones with migration
  targets — so shipping this requires filing the exception explicitly,
  not silently.
- **Hard dependency on BRIDGE-1.** The adapter's suwappu-dag-side mint/
  unlock reuses the `submitHeader` → quorum oracle → mint path that is
  **not yet wired** (`bridge_header.rs:17-22`; README § Bridge
  attestation "the oracle/registry wiring into the mint path is not yet
  live"). Without BRIDGE-1 there is no finalized mint to attach the OFT
  adapter to. This option **cannot start before BRIDGE-1 lands.**
- New contract + audit surface on both the suwappu-dag side and each
  destination endpoint; OFT peer-config and rate-limit management is
  operational work.

### Option B — CCTP-style burn-and-mint for a specific issuer's stablecoin (issuer-only)

Build a Circle-CCTP-shaped burn-and-mint path for **one** registered
issuer's stablecoin: burn on the source, quorum-attested message, mint
native (no wrapped asset) on the destination. This ties directly to the
issuer story — the registered-issuer + reserve-coverage precompiles
(`crates/suwappu-precompiles/{issuer,reserve}`) already model
issuer-scoped mint/burn — and to **FEE-1** (a stablecoin on-chain is the
prerequisite for stablecoin-denominated fees).

**Pros:**
- Native, no wrapped asset — the CCTP property institutions like; matches
  the "no bridge in the path" model Codex shipped (landscape §1).
- Reuses existing primitives. The issuer precompile already enforces
  issuer-only mint/burn; a CCTP-domain message is a natural extension of
  the bridge-header attestation (both are `(state_root)`-anchored quorum
  claims). The burn on suwappu-dag is finalized by the same ML-DSA-65
  quorum-header path — so, like Option A, the suwappu-dag leg can be PQ.
- Directly advances the **issuer/asset gap (G-4)** and unblocks FEE-1.

**Cons:**
- **Issuer-only by construction** — this is CCTP's defining constraint.
  It moves exactly one issuer's asset and nothing else; it does *not*
  give suwappu-dag-native (non-issuer) assets mobility. It is not a
  general interop answer to G-9; it is an issuer-integration deliverable
  wearing an interop hat.
- **Requires a named issuer that does not exist yet.** G-4 is explicit:
  "no issuer using them." Building a CCTP path with no counterparty is
  premature; the USDH lesson (rent distribution, don't build it) applies.
- Same **BRIDGE-1 dependency** as Option A — the destination mint path is
  unwired.
- If Circle's real CCTP ever adds suwappu-dag as a domain, our bespoke
  clone is wasted; we would rather be a CCTP *destination chain* than
  reimplement it.

### Option C — Do nothing / LTP-only (honest baseline)

Ship nothing new. Position LTP as the settlement-attestation layer and
make no claim to asset mobility.

**Pros:**
- Zero new attack/audit surface; no new classical-crypto exception.
- Honest: the "what this is NOT" README voice already concedes we are not
  the retail/liquidity chain (gap analysis §5). Refusing to overclaim
  interop is consistent with that posture.
- Correct *until* BRIDGE-1 lands — there is genuinely nothing to attach an
  adapter to today.

**Cons:**
- **Concedes asset mobility to nobody, because LTP does not move assets.**
  This is the sharp point: "LTP-only" is not a mobility strategy, it is
  the *absence* of one. A suwappu-dag-native asset has no canonical way
  onto another chain. For the institutional-settlement segment we target,
  "your value is stranded on-chain" is a real objection.
- Leaves G-9 fully open and cedes the interop narrative entirely.

### Option D — Rely purely on external aggregators (LI.FI / Squid / deBridge)

Build no adapter of our own; wait until *some* canonical bridge exists
for suwappu-dag assets (via A, B, or a third party) and let aggregators
compose routes over it, exactly as they compose CCTP + OFT + Wormhole NTT
today (landscape §4).

**Pros:**
- Least engineering; leverages the orchestration layer that is already
  the de-facto chain-abstraction surface (landscape §4: "chain
  abstraction is being delivered by these orchestration layers rather
  than by any single standard").
- Aggregator coverage is the actual UX users see, so it is a necessary
  *complement* to A regardless.

**Cons:**
- **Circular / not self-sufficient.** Aggregators compose *existing*
  canonical bridges; they do not create one. There is nothing for LI.FI
  to route over until a canonical path (Option A's OFT adapter, Option
  B's CCTP path, or a third-party OFT deployment) exists. D is a
  downstream consequence of A/B, not an alternative to them.
- No control over the security model or the PQ framing; the classical
  exception is inherited wholesale with no PQ leg to point to.

## Recommendation

**Adopt Option A — position LTP explicitly as the settlement-attestation
layer and add a thin OFT-class adapter for suwappu-dag-native asset
mobility — phased, and gated on BRIDGE-1.** Treat Option D (aggregator
coverage) as the natural follow-on once A exists, and keep Option B
(issuer CCTP path) parked until a named issuer materializes (G-4), at
which point B becomes an issuer-integration deliverable rather than an
interop one.

The reasoning is the gap analysis's own: don't fight the interop-standards
war, plug into it. LTP remains our differentiated *settlement-attestation*
layer (constant-size, PQ where it counts); asset mobility rides the mesh
that already won. Crucially, this keeps the two layers **separate** — the
answer to "are they the same layer or two layers?" is **two layers**: LTP
attests settlement; the OFT adapter moves assets and *consumes* the
quorum-header oracle as its suwappu-dag-side finality source.

This is a **P2 differentiation-defense** item (gap analysis §6), correctly
*behind* the P0/P1 credibility and table-stakes work. It must not start
before **BRIDGE-1** — the `submitHeader` → quorum oracle → mint wiring —
lands, because the adapter has no finalized mint to attach to until then.

### Honest phasing

1. **Phase 0 (doc-only, now):** Ratify the layer boundary. Publish the
   statement "LTP is settlement-attestation, not an asset bridge" in the
   architecture docs and the public positioning. No code. This closes the
   *conceptual* half of G-9 immediately and prevents the conflation error.
2. **Phase 1 (after BRIDGE-1):** Once the quorum-header oracle → mint path
   is live end-to-end, build the OFT adapter contract on the suwappu-dag
   side that burns/locks native asset and reuses the oracle as its
   finality source. File the **classical-crypto exception** for the
   LayerZero transport (mirroring the G-8 BLS exception format) with a
   migration note.
3. **Phase 2 (mesh + aggregators):** Deploy destination OFT peers; get
   listed by LI.FI/Squid (Option D coverage) so users see routes.
4. **Phase 3 (optional, issuer-gated):** If/when a named issuer lands
   (G-4), evaluate Option B's CCTP-style native burn-and-mint for that
   issuer's asset, feeding FEE-1.

## Implementation sketch

Layer boundary and the two artifacts that make it real:

- **LTP stays untouched.** No changes to
  `crates/suwappu-ltp/src/attestation.rs` or the ≈1,600 B commitment
  (`lib.rs:45-46`). The adapter is a *separate* path; it adds zero
  per-payload bytes to the LTP on-chain commitment surface (Invariant 3
  preserved).
- **Adapter finality source = the quorum-header oracle.** The
  suwappu-dag-side burn is finalized by the existing ML-DSA-65
  quorum-header attestation (`bridge_header.rs:108-159`,
  `suwappu_getHeaderAttestation` RPC, README § Bridge attestation). The
  OFT adapter contract gates its `send` on the same
  `SuwappuDagQuorumHeaderOracle.submitHeader` path BRIDGE-1 wires — so the
  adapter is layered *above* the oracle, not beside it. This is the
  concrete answer to "where does the adapter sit relative to the quorum
  header oracle."
- **Escrow model reuse.** The suwappu-dag-side lock/burn accounting can
  reuse the reserved-address escrow pattern already used by the internal
  L1↔L2 bridge (`suwappu-l2-bridge` module doc `lib.rs:5-46`:
  `bridge_escrow_address`, `credit_unchecked`), generalized from
  L1↔own-L2 to L1↔external-mesh. The bridge-accounting invariant
  (`balance(escrow) == sum_of_unwithdrawn`) carries over.
- **PQ exception filing.** Add a documented exception zone for the
  LayerZero transport's classical security model, in the same register as
  the LTP BLS12-381 exception (G-8) — Invariant 2 requires a documented
  migration target, not silence. Framing: *the suwappu-dag leg is
  ML-DSA-65-finalized; the cross-mesh hop is classical.*

## Open questions

1. **OFT vs OFT-adapter vs native OFT.** LayerZero offers a native OFT
   (asset issued as OFT from day one) and an OFT-adapter (wrap an existing
   asset). For a suwappu-dag-native asset that already exists on L1, the
   adapter/lockbox variant is the fit — but this needs confirmation
   against the current LayerZero contracts.
2. **DVN configuration.** Do we run our own LayerZero DVN (to keep a foot
   in the security model) or accept the default DVN set? Running one is
   the only way to reduce the classical-trust surface; it is operational
   cost. Open.
3. **Does the OFT adapter's suwappu-dag-side finality *have* to be the
   quorum-header oracle,** or can LayerZero's own DVN observe suwappu-dag
   finality directly? Reusing the oracle is what preserves the PQ leg;
   confirm it is compatible with LayerZero's message-verification flow.
4. **Interaction with the internal L1↔L2 bridge.** Should an asset be able
   to move L1 → external-mesh directly, or only after materializing on L1
   from the zk-L2? Routing/ordering semantics need a decision.
5. **Issuer-path timing (Option B).** Is there any near-term issuer
   candidate that would make the CCTP path worth pulling forward ahead of
   the generic OFT adapter? Tied to G-4.
6. **When does Circle/CCTP add chains?** If suwappu-dag can become a CCTP
   *destination domain* upstream, that dominates building a bespoke
   CCTP clone (Option B). Track Circle's chain-onboarding roadmap.

## Decision

**Pending sign-off.** Recommendation is Option A (LTP-as-attestation +
thin OFT-class adapter), phased, and **explicitly gated on BRIDGE-1**
landing the `submitHeader` → quorum oracle → mint path first. Phase 0
(ratify the layer boundary; publish "LTP is settlement-attestation, not an
asset bridge") can proceed on sign-off with no code dependency; Phases 1-3
are blocked on BRIDGE-1.

## See also

- [`docs/research/competitive-gap-analysis.md`](../research/competitive-gap-analysis.md) —
  gap **G-9** (interop proprietary) + workstream **INTEROP-1**; and the
  **BRIDGE-1** dependency (§6, P1).
- [`docs/research/briefs/landscape.md`](../research/briefs/landscape.md) §4 —
  CCTP v2 + LayerZero OFT as the winning cross-chain standards.
- [`docs/architecture/ltp-integration.md`](../architecture/ltp-integration.md) —
  LTP Commit/Lattice/Materialize + the constant-size commitment.
- [`crates/suwappu-ltp/src/attestation.rs`](../../crates/suwappu-ltp/src/attestation.rs) —
  the 7-of-9 corridor attestation (settlement-attestation, not asset move).
- [`crates/suwappu-consensus/src/bridge_header.rs`](../../crates/suwappu-consensus/src/bridge_header.rs) —
  the source-side ML-DSA-65 quorum-header attestation (oracle UNFED; mint
  path not wired — the BRIDGE-1 surface).
- [`crates/suwappu-l2-bridge/src/lib.rs`](../../crates/suwappu-l2-bridge/src/lib.rs) —
  the internal L1↔L2 escrow bridge (payload validation; not external interop).
- `README.md` § Bridge attestation — the honest "mint path not yet wired"
  framing.
- **Invariant 2** (PQ-conservative crypto surface) — the exception-zone
  requirement the OFT transport's classical model triggers.
- **Invariant 3** (constant-size LTP commitment) — preserved; the adapter
  is a separate path.
