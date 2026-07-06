# IQ-008 — EVM developer surface (eth_* compat vs Intent-SDK-only)

**Status:** Recommendation, pending sign-off. Blocks the EVM-1
workstream in the competitive gap analysis; no code lands until this
IQ is ratified.
**Owner:** RPC / execution / developer-relations
**Date:** 2026-07-03
**Tracking:** `docs/research/competitive-gap-analysis.md` gap G-3 /
workstream **EVM-1** (§3, §6). Related: FEE-1 (fee abstraction),
BRIDGE-1 (destination-EVM bridge verifier).

## Question

Every funded competitor in the payments/settlement category (Tempo,
Arc, Robinhood Chain, Solana-via-Neon) ships an Ethereum-standard
developer surface on day one: an `eth_*` JSON-RPC endpoint that
MetaMask, Foundry, Hardhat, ethers/viem, and the entire EVM tooling
graph can target unmodified. suwappu-dag today exposes a proprietary
`suwappu_*` JSON-RPC surface and Rust/TS SDKs that wrap it. The
dual-VM (EVM/Move) work is a **read-only projector** over the
polymorphic balance map, not an execution endpoint.

**Should suwappu-dag expose an Ethereum-standard developer surface
(`eth_*` JSON-RPC so MetaMask/Foundry/Hardhat can target it), or
commit to the Intent-SDK-only model — and what is the trade-off?**

This is **EVM-1** from the gap register.

### What exists today (grounded in the repo)

The current JSON-RPC surface is **exclusively `suwappu_*`** — nine
methods, dispatched in `crates/suwappu-rpc/src/router.rs:178-190`:

```
suwappu_getEpoch, suwappu_getAuthorityRegistry, suwappu_getValidatorRegistry,
suwappu_getStake, suwappu_getBalance, suwappu_getBlock, suwappu_getTransaction,
suwappu_getHeaderAttestation, suwappu_submitIntent
```

There is **no `eth_*` method anywhere** in `crates/` or `clients/`
(grep for `eth_[a-zA-Z]` returns zero hits). The only write path is
`suwappu_submitIntent` (`crates/suwappu-rpc/src/methods.rs:238-285`),
which takes a **bincode-serialized `suwappu_execution::Intent` + a
detached ML-DSA-65 signature + a `blake3(pubkey)` signer hash** — not
an RLP-encoded, ECDSA-signed Ethereum transaction. Both SDKs
(`clients/ts-sdk/src/index.ts`, `clients/rust-sdk/src/lib.rs`) are
thin wrappers over these same `suwappu_*` methods.

The "EVM" in this system appears in two distinct, non-endpoint places:

1. **Dual-VM read-only projector** (Paper §7.1, `docs/architecture/execution.md`):
   EVM `balanceOf` and Move `Coin::value` read the *same* canonical
   balance map via a projector surface that does **not** go through
   either VM's mutation path. Mutation happens through the Intent
   surface (`Intent::Transfer`, `Intent::Call`, Move variants —
   `crates/suwappu-execution/src/substrate.rs:103`), threaded through
   the suwappu-db `BlockExecutor`. The EVM is a *view*, not a
   transaction acceptor.
2. **Destination-EVM bridge verifier** (`suwappu-revm`,
   `crates/suwappu-mldsa-precompile/`, README §Bridge): a full EVM on
   *another* chain that verifies suwappu-dag's ML-DSA-65 header
   attestations via precompiles `0x0101` (ML-DSA-65) and `0x0102`
   (BLAKE3). This EVM is a consumer of suwappu-dag finality, not part
   of suwappu-dag's own developer surface.

So the honest current state: **no `eth_*` RPC, no Ethereum
transaction ingress, and the EVM projection is a read-only balance
view — not a MetaMask/Foundry-compatible endpoint.**

### The crypto-invariant tension (key point)

Invariant 2 (PQ-conservative crypto surface) is the headline
differentiator per the gap analysis: ML-DSA-65 is the **primary**
signing surface, and classical primitives (ECDSA secp256k1,
BLS12-381) are retained only on documented exception zones with
migration targets. An Ethereum transaction is, by definition, an
**ECDSA secp256k1**-signed, RLP-encoded object. A real
`eth_sendRawTransaction` endpoint therefore reintroduces exactly the
classical signing surface Invariant 2 is designed to minimize — and
does so on the *primary user-facing write path*, the most load-bearing
surface of all. This is the central trade-off, not an incidental one:
the EVM developer surface and the PQ-by-default thesis are in direct
tension on the signing primitive.

## Options surveyed

### Option A — Full `eth_*` compatibility shim + tx-format adapter

Stand up an `eth_*` JSON-RPC facade over the existing EVM projection,
plus a **transaction-format adapter** that decodes Ethereum
transactions (RLP + ECDSA recovery) and maps them onto Intents:
`eth_sendRawTransaction` → decode → synthesize the equivalent
`Intent::Transfer`/`Intent::Call` → enqueue. Add the read methods
(`eth_call`, `eth_getBalance`, `eth_blockNumber`, `eth_getBlockBy*`,
`eth_getTransactionReceipt`, `eth_chainId`, `eth_gasPrice`,
`eth_estimateGas`, `eth_getTransactionCount`) mapped onto the existing
`suwappu_*` state views and the projector. MetaMask/Foundry/Hardhat
point at it unmodified.

**Pros:**
- Closes G-3 at parity: the entire EVM tooling graph works day one,
  which is the single most-repeated table-stake in the category.
- "Meet developers where they are" — zero SDK-adoption friction; every
  ethers/viem tutorial, wallet, and block explorer works.
- Reuses the read projector already in the substrate for the
  view-method half (`eth_call`/`eth_getBalance` map cleanly onto the
  read-only balanceOf projection).

**Cons:**
- **Reintroduces classical crypto on the primary write path
  (Invariant 2 violation in spirit).** `eth_sendRawTransaction`
  carries an ECDSA secp256k1 signature. To honor Invariant 2 the
  adapter would have to *re-sign* the mapped Intent with an ML-DSA-65
  key it custodies — meaning the node holds user signing authority
  (a custodial anti-pattern), OR the chain accepts ECDSA on the write
  path and the PQ-by-default headline becomes "PQ-by-default except
  for the EVM ingress," which is precisely the rebuttal the gap
  analysis warns about (cf. the BLS12-381 exception, G-8).
- **Semantic impedance mismatch is deep, not cosmetic.** Ethereum tx
  semantics — sequential per-account **nonces**, **gas**/gasPrice/gas
  limit metering, `chainId`-scoped EIP-155 replay protection, the
  21000-gas floor, `CREATE`/`CREATE2` address derivation, event
  logs/bloom filters, and receipt `status` — have no native analogue
  in the Intent model. suwappu-dag has no native gas token surface
  (gap G-2: "no fee abstraction"), no per-account nonce (Intents are
  content-hashed via `blake3(bincode(intent))`, not nonce-ordered),
  and no EVM log/receipt substrate. Every one of these has to be
  emulated convincingly enough that Foundry's assertions pass —
  a large, permanently-maintained compatibility surface.
- **The projector is read-only.** `eth_call` against the projection is
  feasible; a *state-changing* `eth_call`/contract deployment is not,
  because the EVM here does not execute — it projects. Full Foundry
  `forge test` against a forked state expects an executing EVM.
  Delivering that means promoting the projector to an execution VM,
  which is a much larger change than an RPC shim.
- Audit surface expands materially (RLP decoder, ECDSA recovery,
  nonce/gas emulation) right as the Trail of Bits consensus audit
  (Track A.2) is scoped.

### Option B — Intent-SDK-only (RECOMMENDED)

Do **not** chase EVM developers. Keep the `suwappu_*` JSON-RPC + the
Rust/TS SDKs as the sole developer surface, and compete explicitly on
the PQ / dual-ring-settlement thesis. Document the decision publicly
and pair it with the positioning recommendation in the gap analysis
(§5): institutional/regulated settlement, not retail dApp deployment.

**Pros:**
- **Preserves Invariant 2 cleanly.** ML-DSA-65 stays the *only*
  primary signing surface; there is no ECDSA write path to explain
  away. The PQ-by-default headline stays intact and unqualified —
  which is the one differentiator no competitor can currently claim
  (gap analysis §4.1).
- **Matches the honest positioning already chosen.** The gap analysis
  §5 says explicitly: "Concede explicitly: we are not EVM-first, not
  retail... An honest 'what this is NOT' section already exists in the
  README — keep that voice." Option B *is* that concession made
  load-bearing. suwappu-dag is a settlement chain for regulated value,
  not a smart-contract L1 competing for dApp mindshare.
- **No semantic-emulation debt.** No nonce/gas/receipt/log emulation,
  no RLP/ECDSA decode path, no custodial re-signing question, minimal
  new audit surface. Engineering stays pointed at the P0 credibility
  gaps (perf, bridge, confidential transfers) that actually block the
  institutional pitch.
- The Intent surface is *stronger* for the target audience: typed
  Intents with explicit compliance semantics (DID, registered-issuer,
  reserve-coverage precompiles) are more auditable than opaque EVM
  calldata — an asset with auditors/regulators, the intended buyers.

**Cons:**
- **Concedes G-3 at parity, permanently.** No MetaMask, no Foundry, no
  Hardhat, no ethers/viem, no drop-in block-explorer integrations.
  Every integrator writes against the proprietary SDK. This is a real
  adoption tax and narrows the top of the funnel to teams willing to
  learn a new surface.
- Reads as "smaller ecosystem" to anyone benchmarking on tooling
  breadth; the decision must be argued, not defaulted, or it looks
  like a gap rather than a choice.
- If the strategy later pivots toward retail/dApp distribution, this
  becomes a hard blocker that Option A/C would have pre-empted.

### Option C — Hybrid: read-only `eth_*` projection, no `eth_sendRawTransaction`

Expose the **read half** of the EVM RPC surface only —
`eth_call` (view/pure only), `eth_getBalance`, `eth_blockNumber`,
`eth_chainId`, `eth_getBlockByNumber`, `eth_getTransactionReceipt`,
`eth_getCode` — mapped onto the existing read-only projector and the
`suwappu_*` state views. **Do not** expose `eth_sendRawTransaction`,
`eth_sendTransaction`, or any write path; writes stay on
`suwappu_submitIntent` with ML-DSA-65. Move/EVM contract *execution*
stays exactly as today (projector, not executor).

**Pros:**
- **Preserves Invariant 2 fully** — no ECDSA anywhere on the write
  path, because there is no EVM write path. The classical-crypto
  tension of Option A is avoided entirely.
- **Wallets and explorers can *read* the chain** via standard tooling:
  MetaMask can display balances, block explorers built on
  Ethereum-RPC assumptions can index it, dashboards work. This is a
  meaningful ergonomics win at low invariant cost.
- Maps cleanly onto what already exists: the projector *is* read-only,
  so `eth_call`/`eth_getBalance` are honest projections, not
  emulations of execution.

**Cons:**
- **"Half-compatible" is a known footgun.** A wallet that can read but
  gets an error (or silent failure) on `eth_sendRawTransaction`
  produces a worse first impression than no `eth_*` at all — users
  connect MetaMask, see a balance, try to send, and it breaks. The
  compatibility promise is implicitly a *write* promise to most users.
- Still incurs the read-side emulation debt: `eth_getTransactionReceipt`
  expects EVM receipt/log/status shape that the Intent model doesn't
  produce natively; faking it convincingly is non-trivial and a
  standing maintenance + audit cost for a partial win.
- Doesn't actually close G-3 (Foundry/Hardhat need the write path);
  it's a UX nicety, not developer-surface parity. Risks being read as
  the worst of both — invariant-clean but not tooling-complete, while
  still carrying emulation cost.

## Recommendation

**Option B — Intent-SDK-only**, with the decision documented publicly
as a positioning choice rather than a gap, and **Option C's read-only
`eth_*` projection deferred as a v1.1+ ergonomics option** if
institutional integrators specifically ask for standard-RPC read
access.

This is not a reflexive "add EVM" — the honest read of the gap
analysis is that suwappu-dag's defensible lane (§5) is
*post-quantum settlement for regulated value*, and in that lane the
EVM developer graph is not the buyer. The buyers are
Canton/Kinexys/Fnality-class institutions for whom PQ compliance
(CNSA 2.0, 2027-01-01) and auditability decide wins — not
dApp-deployment ergonomics. Chasing Option A would spend the scarce
engineering budget re-litigating Invariant 2 (the *one* thing no
competitor can claim) in order to reach parity on a table-stake that
matters most to the audience suwappu-dag has explicitly decided *not*
to fight for (retail/dApp distribution, already consolidated by
Tempo/Arc/Solana).

The decisive argument is the crypto-invariant tension: **Option A
reintroduces ECDSA secp256k1 on the primary user write path — exactly
the classical primitive Invariant 2 exists to minimize — and forces a
choice between a custodial re-signing anti-pattern and a qualified
"PQ-except-for-EVM" headline that hands analysts the same rebuttal the
BLS12-381 exception already invites (G-8).** Neither is acceptable for
a chain whose entire differentiation is PQ-by-default on primary
surfaces.

If the strategy ever pivots to retail/dApp distribution, revisit this
IQ — Option A becomes a genuine requirement, and the migration path is
the tx-format adapter + a decision on the ECDSA write-path exception
(which would itself need a new IQ against Invariant 2).

## Implementation sketch

For the recommended path (Option B), the "implementation" is largely
**documentation + a public positioning artifact**, not code:

1. **Document the decision as a stance.** Extend the README's existing
   "what this is NOT" section with an explicit line: suwappu-dag
   exposes a typed Intent surface signed with ML-DSA-65; it is *not*
   an `eth_*`-compatible endpoint, by design, to keep PQ-by-default on
   the primary write path. Frame it as the invariant-2 corollary it
   is.
2. **Harden and document the Intent SDK surface** so the proprietary
   surface is a strength, not a rough edge: complete the
   `clients/{rust-sdk,ts-sdk}` reference docs (typedoc/rustdoc already
   scaffolded), publish an Intent cookbook (transfer, mint via
   registered-issuer, reserve-coverage check), and make
   `suwappu_submitIntent` ergonomics first-class (client-side Intent
   construction + ML-DSA-65 signing helpers).
3. **Keep Option C on the shelf, spec'd but unbuilt.** If an
   institutional integrator asks for standard-RPC *read* access, the
   read-only `eth_*` projection is a bounded addition: a new dispatch
   arm in `crates/suwappu-rpc/src/router.rs` mapping
   `eth_getBalance`/`eth_blockNumber`/`eth_chainId`/`eth_getBlockByNumber`
   onto the existing `StateView` methods and the projector. Explicitly
   return a structured "unsupported: this chain accepts ML-DSA-65
   Intents via suwappu_submitIntent" error for every write method, so
   the boundary is honest and never silently fails.

For reference, had Option A been chosen, the write-path shape would be:
`eth_sendRawTransaction(rlp)` → RLP decode → ECDSA secp256k1 recover
sender → map to `Intent::Transfer`/`Intent::Call` → **[unresolved:
sign with what key?]** → enqueue via the existing intent channel. The
bracketed step is exactly the invariant tension and is why Option A is
not recommended without a separate Invariant-2 exception IQ.

## Open questions

1. **Read-only `eth_*` trigger (Option C).** What concrete integrator
   ask flips Option C from "shelved" to "build"? A named block
   explorer or custody dashboard that requires Ethereum-RPC read
   assumptions is the likely trigger — define the bar so it's a
   decision, not drift.
2. **Receipt/log shape.** Even a read-only `eth_*` projection must
   decide how (or whether) to synthesize `eth_getTransactionReceipt`
   from the Intent/`ExecutionReport` model. Options: omit it (return
   unsupported), or map committed-Intent → a minimal receipt shape.
   Needs its own mini-spec if Option C is ever built.
3. **Interaction with FEE-1 (G-2).** If stablecoin-denominated /
   sponsored fees land (FEE-1), does a fee-payer≠sender Intent field
   change the calculus for an EVM adapter? Track the dependency; do
   not couple the two decisions.
4. **`chainId` allocation.** Even Option C's `eth_chainId` needs a
   registered EIP-155 chain id. Reserve one now (cheap, avoids a
   collision later) even if the read projection is deferred.

## Decision

**Pending sign-off.**

Recommendation: **Option B — Intent-SDK-only, documented as a
positioning choice**, with Option C's read-only `eth_*` projection
spec'd but deferred to a concrete institutional-integrator trigger.
Ratify alongside the public positioning in
`docs/research/competitive-gap-analysis.md` §5 so the "we are not
EVM-first" concession and this IQ are a single, coherent public stance.

## See also

- [`docs/research/competitive-gap-analysis.md`](../research/competitive-gap-analysis.md) —
  gap G-3 / workstream EVM-1 (§3, §6), positioning (§5).
- [`docs/architecture/execution.md`](../architecture/execution.md) —
  the dual-VM read-only projector (Paper §7.1).
- [`crates/suwappu-rpc/src/router.rs:178-190`](../../crates/suwappu-rpc/src/router.rs) —
  the `suwappu_*`-only dispatch table (no `eth_*`).
- [`crates/suwappu-rpc/src/methods.rs:238-285`](../../crates/suwappu-rpc/src/methods.rs) —
  `suwappu_submitIntent` (ML-DSA-65 + bincoded Intent write path).
- [`crates/suwappu-execution/src/substrate.rs:103`](../../crates/suwappu-execution/src/substrate.rs) —
  the `Intent` enum (Transfer / Call / Move variants).
- README §Bridge — `suwappu-revm` / `suwappu-mldsa-precompile`: the
  destination-EVM bridge verifier (address `0x0101` ML-DSA-65,
  `0x0102` BLAKE3) — an EVM *consumer* of suwappu-dag, not its
  developer surface.
- CLAUDE.md Invariant 2 (PQ-conservative crypto surface) — the
  invariant this IQ is decided against.
</content>
</invoke>
