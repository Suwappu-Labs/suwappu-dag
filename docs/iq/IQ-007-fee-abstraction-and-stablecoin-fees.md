# IQ-007 — Fee abstraction: stablecoin-denominated + sponsored fees

**Status:** Recommendation, pending sign-off. Phased; Phase 1
(fee-payer/sender separation) is buildable today, Phase 2
(stablecoin-denominated fees) is gated on the issuer story (gap G-4).
**Phase-1 primitive landed** (2026-07-03): the sponsorship signing
surface, wire envelope, fee-derived mempool priority, and the atomic
`settle_protocol_fee` / `apply_intent_with_fee` / `execute_block_with_fees`
substrate primitives are implemented and reviewed (crypto-reviewer +
lane-auditor **APPROVE-WITH-NITS**). The path is **dormant** —
`execute_block` still delegates with an empty fee slice, so no funds move
yet. See "Implementation status" below.
**Owner:** Execution / payments
**Date:** 2026-07-03
**Tracking:** Refs [`../research/competitive-gap-analysis.md`](../research/competitive-gap-analysis.md)
FEE-1 (gap G-2, §3 register + §6 P1 workstream).

## Implementation status (2026-07-03)

**Landed (Phase 1, reviewed, dormant):**

- Client wire: `ClientMessage::SubmitWithFee` / `SubmitBatchWithFee`
  (appended variants, bincode-compatible), `FeeAuthorization`,
  `FEE_DOMAIN_TAG` + `fee_signing_digest` (binds intent content-hash +
  `max_fee` + `network_id`), `verify_signed_fee` (reuses the audited
  ML-DSA-65 `verify_authority_signature`).
- Mempool: fee-derived admission priority replaces `DEFAULT_INTENT_PRIORITY`
  when a validated `fee_payer` is present.
- Substrate: `FeeCharge`, `Substrate::{apply_intent_with_fee,
  settle_protocol_fee}` (atomic fee-first + reverse-transfer rollback;
  reserved-payer guard; zero-fee guard; fail-closed default),
  `execute_block_with_fees` (length-aligned; `execute_block` delegates
  with `&[]`). Fee sink = `authority_rewards_pool`.
- Tests: unit (reserved-payer reject, zero-fee reject, insufficient-sponsor
  atomicity, refund-on-intent-failure, misaligned-fees panic) +
  `proptest_fee_settlement.rs` (fee-less parity, atomicity, supply
  conservation — InMemory only).

**Hard prerequisites for the settlement PR (before any fee moves real funds):**

1. **Nonce / expiry / revocation** on `FeeAuthorization` (crypto-reviewer
   MED). Today the authorization is a content-bound bearer token with no
   replay defense of its own beyond mempool content-hash dedup — acceptable
   only while dormant.
2. **Thread the envelope mempool → block** so `execute_block_with_fees`
   receives real `FeeCharge`s (needs a mempool `Entry` change, out of the
   Phase-1 blast radius).
3. **Full 10k-case mock-vs-prod parity proptest** (`InMemorySubstrate` vs
   `SuwappuDbSubstrate`) — deferred because suwappu-db is unfetchable in the
   dev sandbox; must run in CI.
4. **Surface (not swallow) the rollback error** on the SuwappuDb path
   (`let _ =` → observable) so a suwappu-db Transfer-contract regression
   can't silently mis-account the fee ledger.
5. Multi-intent block-level failure-injection proptest.

These are recorded here and as inline `SETTLEMENT-PR-PREREQUISITE`
comments at the `SubmitWithFee` handler and `apply_intent_with_fee`.

## Question

Every funded competitor in the payments/settlement category ships
gasless or stablecoin-denominated fees as a *table stake*: Tempo has no
native token at all — fees are USD-denominated, paid in stablecoins,
default-routed to pathUSD via a cascading fee-token selector, with a
protocol-level fee AMM converting the payer's stablecoin into the
validator's preferred one, and a *second signature* that separates the
logical sender from the fee payer so apps can sponsor user gas. Arc uses
USDC as gas (EWMA base fee + ceiling) with Circle Paymaster for
sponsorship. Holding a volatile native token to move dollars is now
read as disqualifying — and it is doubly disqualifying for our own
institutional-settlement pitch, since regulated counterparties will not
custody a volatile gas asset.

**suwappu-dag has no fee market today.** This is the honest baseline,
confirmed against the code:

- The `Intent` enum
  ([`crates/suwappu-execution/src/substrate.rs:103`](../../crates/suwappu-execution/src/substrate.rs),
  `#[non_exhaustive]`) carries **no** fee, gas, tip, or fee-payer field
  on any variant. `Transfer` moves `amount` and nothing is charged for
  the move. The only `gas`/`fee` string anywhere in `substrate.rs` is a
  comment about *L1* gas in the force-include path (`:2336`).
- Every intent submitted over the client wire lands at
  `DEFAULT_INTENT_PRIORITY = 0`
  ([`crates/suwappu-node/src/client.rs:510`](../../crates/suwappu-node/src/client.rs));
  the comment there already flags the intended retirement of the
  constant "when the fee surface lands (S34+)". Mempool ordering is
  therefore pure FIFO by `submit_ms`
  ([`crates/suwappu-mempool/src/mempool.rs:97`](../../crates/suwappu-mempool/src/mempool.rs),
  the `(Reverse(priority), submit_ms, hash)` key), even though
  `Mempool::submit` already takes an opaque `priority: u64` it never
  receives a non-zero value for.
- Client submissions carry exactly **one** ML-DSA-65 detached signature
  over `intent_signing_digest(network_id, intent)`
  (`ClientMessage::Submit { intent, signature, signer_pubkey_hash }`,
  `client.rs:167`). There is no sender/fee-payer distinction, and the
  signer must be a **seated Authority Ring member** — the Validator Ring
  registry carries no pubkey material yet, and the devnet collapses
  `owner == submitter` (see the fast-path owner-binding note,
  `client.rs:718-738`). There is no general account/nonce model.

So the question is two questions:

1. **Should the `Intent` surface gain fee-payer ≠ sender separation plus
   a sponsorship (paymaster-style) authorization?**
2. **Should fees be denominated in a stablecoin (a registered-issuer
   asset) rather than native SUWAPPU, and if so, how does that ride the
   existing issuer + reserve-coverage precompiles?**

Four options were surveyed. None map onto the current surface without
change; the current surface has no fee concept at all.

## Options surveyed

### Option A — Fee-payer/sender separation in the Intent envelope + a sponsorship signature (RECOMMENDED, Phase 1)

Model on Tempo's account-abstraction design: leave the `Intent`
*payload* untouched (it still describes what happens — the transfer, the
mint, the delegation) and add a **fee envelope** at the submission
layer. The envelope names a `fee_payer` distinct from the intent's
logical sender and carries a **second ML-DSA-65 signature** from that
payer authorizing "I will pay up to `max_fee` for this exact intent."
Unsponsored intents omit the envelope and the sender pays, exactly as
Tempo's second signature is optional. Fees stay denominated in SUWAPPU
in this phase — the stablecoin question (Option B) is deferred.

**Pros:**

- **Minimal blast radius on the Intent semantics.** The `#[non_exhaustive]`
  `Intent` enum (`substrate.rs:103`) does not need a fee field per
  variant; the envelope lives on `ClientMessage::Submit`
  (`client.rs:167`) and in the mempool `Entry`. Intent content-hashing
  (`blake3(bincode(intent))`, `mempool.rs:158`) is unchanged, so dedup
  and the `tx_to_block` index keep working.
- **The signing-digest machinery already exists and is the right shape.**
  `intent_signing_digest` and `fastpath_signing_digest`
  (`client.rs:125,145`) are two domain-tagged blake3 digests over
  `TAG || network_id || bincode(...)`, each verified through one shared
  `verify_authority_signature`. A `fee_signing_digest` under a new
  `SUWAPPU_FEE_V1` tag, bound to the *intent hash* + `max_fee`, is a
  third instance of an established pattern, and a `verify_signed_fee`
  wrapper mirrors `verify_signed_intent` / `verify_signed_fastpath`
  one-for-one. The two-signatures-never-cross-replay property is already
  enforced by domain-tag separation here.
- **Stays on the PQ-conservative surface (invariant 2).** Both the
  sender and sponsor signatures are ML-DSA-65. Every competitor's
  account-abstraction/sponsorship signature is classical (ECDSA
  secp256k1); a PQ-native paymaster path is a differentiator that falls
  out for free, not a compromise.
- **Fee settlement is one more balance-map mutation, so it inherits the
  substrate invariants.** A fee debit of `fee_payer` (or `from`) plus a
  credit to a reward/treasury address flows through the same
  `apply`-path bundle as the intent, so it is **bundle-atomic**,
  **dual-VM projection-equal**, and **schedule-deterministic** by
  construction (invariant 4). It does not need its own tree or its own
  commit.
- **It does not touch the joint-quorum AND-gate (invariant 1).** Fees
  settle at *execution*, after Mysticeti-C linearization and the
  dual-ring joint-quorum commit. The fee surface never gates commit on
  payment, so it cannot entangle the two rings. (Any design that made
  commit conditional on fee payment would couple the rings and must be
  rejected — see Open questions.)
- **Turns the dormant priority knob live.** Replacing
  `DEFAULT_INTENT_PRIORITY = 0` with a fee-derived priority feeds
  `Mempool::submit`'s existing `priority: u64` a real value; no mempool
  data-structure change (the `Reverse(priority)` key already sorts
  fee-first). This is a prerequisite for a spam-resistant public RPC
  anyway.

**Cons:**

- **Two ML-DSA-65 signatures per sponsored tx ≈ 6.6 KB.** One ML-DSA-65
  signature is ~3,309 B; a sponsorship doubles it. Against a classical
  ~65 B ECDSA second signature this is ~100× the sponsorship overhead,
  and it presses on `MAX_FRAME_BYTES` and the mempool's 10,000-entry
  capacity (`MempoolConfig::capacity`, `mempool.rs:29`). This is the PQ
  tax; it is real and should be measured before committing to
  always-detached sigs.
- **Needs an account/nonce model the devnet lacks.** Today only seated
  Authorities submit and `owner == submitter`. A fee payer who is *not*
  the sender, and is not necessarily an Authority, requires (a)
  extending the auth gate beyond the Authority Ring — the exact Issue
  #28 follow-up that `verify_authority_signature` (`client.rs:470`) is
  waiting on — and (b) a replay-nonce on the sponsorship leg so one
  signed "I'll pay `max_fee` for intent H" cannot be reused. Binding the
  fee digest to the intent's content hash closes cross-intent replay but
  not same-intent resubmission; a nonce or the mempool's existing
  content-hash dedup has to cover the rest.
- **Fee accounting policy is unspecified.** SUWAPPU-denominated fees
  still need a unit (flat tip vs. gas×price) and a recipient (proposer
  vs. `treasury_address` vs. burn), which interacts with the tokenomics
  Intents (`MintInflation`, `DistributeRewards`).

### Option B — Stablecoin-denominated fees via a registered-issuer fee token + conversion (RECOMMENDED, Phase 2, gated)

Model on Arc USDC-gas and Tempo's fee AMM: denominate fees in a
registered-issuer asset (an `AssetId` from
[`crates/suwappu-precompiles/src/issuer.rs:42`](../../crates/suwappu-precompiles/src/issuer.rs))
rather than SUWAPPU, and convert the payer's fee asset into the
validator's preferred asset via a protocol fee-AMM step.

**Pros:**

- **This is the actual category table stake.** FEE-1 / gap G-2 exists
  because "holding a volatile native token to move dollars is now
  disqualifying." Only stablecoin denomination — not merely sponsorship —
  fully closes it.
- **The primitives already exist in-repo.** The registered-issuer
  precompile (`issuer.rs`, mint/burn, per-issuer `AssetId`) and the
  reserve-coverage circuit breaker (`reserve.rs`, `CoverageRule`,
  `ReserveCoverageChecker`) are exactly the "is this a sound
  dollar-token" machinery a fee token needs. The suwappu-db balance map
  is already polymorphic/multi-asset, so a per-asset fee debit is
  feasible at the substrate level.
- **Fee-AMM conversion is expressible as an Intent arm**, keeping the
  validator-preferred-asset routing on-chain and deterministic rather
  than an off-protocol side channel.

**Cons:**

- **The precompiles are not wired into `apply_intent` today.** Per the
  IQ-006 audit finding, `issuer` / `reserve` are standalone validation
  modules with no dispatch integration; `apply_intent` never calls them.
  Charging a fee in an issuer asset requires the *same* dispatch-surface
  work Track G's L2 verifier precompile needs — a non-trivial lift, not
  a field add.
- **There is no stablecoin on the chain (gap G-4).** The issuer and
  reserve precompiles have no issuer using them. You cannot denominate
  fees in an asset that no one has minted, so Option B is **blocked on
  the issuer story** and cannot ship standalone. Phase 2 is real only
  once G-4 lands.
- **A fee AMM introduces a price/oracle trust surface** (what is the
  fee-asset→preferred-asset rate?) that does not exist anywhere in the
  repo today and would need its own IQ-grade treatment, including how it
  interacts with reserve-coverage pauses (a paused mint on the fee asset
  must not brick the fee market).
- **The Intent surface + mempool priority are implicitly single-asset.**
  Fee-derived priority (Option A) assumes one comparable unit; multi-asset
  fees need a common numeraire to order the mempool, which reintroduces
  the oracle question at admission time.

### Option C — Protocol-level paymaster account

A reserved protocol account (in the style of the existing
`treasury_address` / `insurance_pool_address` reserved accounts in
[`crates/suwappu-execution/src/reserved.rs`](../../crates/suwappu-execution/src/reserved.rs))
pays fees for a whitelisted class of intents. No envelope change, no
second signature.

**Pros:**

- **Simplest possible gasless UX.** Good for a faucet-backed public
  devnet / testnet where the goal is "users transact without holding
  SUWAPPU" (supports the LAUNCH-1 / G-3 faucet work). A reserved
  paymaster account is a well-trodden pattern here.
- **No wire bloat** — no second ML-DSA-65 signature, so none of Option
  A's ~6.6 KB cost.

**Cons:**

- **It is not app-directed sponsorship.** A dApp cannot selectively
  sponsor *its own* users; sponsorship becomes a governance/protocol
  policy decision, not an application capability. This is strictly
  weaker than Tempo/Circle Paymaster, which is what FEE-1 asks for.
- **It does not deliver fee-payer ≠ sender in the Intent surface** — the
  explicit FEE-1 deliverable. It is a convenience, not the answer.
- **Centralizes and politicizes sponsorship** (who is on the whitelist?)
  and invites abuse/drain of the reserved account without per-app
  accounting.

### Option D — Do nothing / keep SUWAPPU-only (the honest baseline)

Ship nothing; fees remain absent and, when a fee market eventually
lands, native-SUWAPPU-only.

**Pros:**

- **It is the current, truthful state** — zero work, zero new attack
  surface, and consistent with the "concede explicitly: not EVM-first,
  not retail" positioning in the gap analysis (§5).
- No new signing surface to audit before the Trail of Bits consensus
  audit.

**Cons:**

- **Leaves a Critical (G-2) category table stake open at public
  launch.** Requiring a volatile native token to move dollars is
  disqualifying for retail *and* for the regulated-settlement audience
  we are actually targeting — banks will not hold a volatile gas asset
  either.
- The competitive delta is not narrowing: Tempo (mainnet) and Arc
  (testnet, mainnet summer 2026) both ship this today. Do-nothing is the
  baseline to beat, not a plan.

## Recommendation

**Phased: Option A now, Option B when the issuer story lands; Option C
only as a narrow devnet convenience; never Option D.**

- **Phase 1 (P1, target before the incentivized testnet / Phase 5):
  Option A.** Add fee-payer ≠ sender separation and an ML-DSA-65
  sponsorship signature to the submission envelope, with fees
  SUWAPPU-denominated, and replace `DEFAULT_INTENT_PRIORITY` with a
  fee-derived priority. This delivers the *shape* of gasless/paymaster
  UX — the part FEE-1 names explicitly ("fee-payer ≠ sender separation +
  a fee-sponsorship path") — without a dependency on any issuer. It is
  buildable against today's code.
- **Phase 2 (gated on gap G-4, the issuer story, and on `apply_intent`
  precompile dispatch shared with Track G): Option B.** Once a
  registered issuer is live on the `issuer` precompile with
  reserve-coverage attestation, allow fees denominated in that
  `AssetId`, converted via a fee-AMM Intent arm. Do not attempt Option B
  before G-4; there is nothing to denominate fees in.
- **Option C** ships opportunistically as a reserved paymaster account
  for the faucet/devnet gasless path only — it is not the FEE-1 answer
  and must not be positioned as such.
- **Option D is rejected** as a shipping posture, though it remains the
  accurate description of *today*.

Do not overstate this as parity. Even Phase 1 + Phase 2 shipped is
"credible fee abstraction," not "Tempo/Arc parity" — we would still lack
their issuer distribution and their fee-AMM maturity. The claim to make
publicly is "PQ-native sponsored + stablecoin fees," not "we match
Circle Paymaster."

## Implementation sketch

Named crates/types that change. `#[non_exhaustive]` on `Intent`
(`substrate.rs:103`) means any new Intent arm needs the full consumer
`-p` set per CLAUDE.md (`suwappu-execution suwappu-node suwappu-rpc
suwappu-fastpath suwappu-mempool`).

**Phase 1 (Option A):**

1. **Fee envelope on the client wire** —
   [`crates/suwappu-node/src/client.rs`](../../crates/suwappu-node/src/client.rs).
   Extend `ClientMessage::Submit` (and `SubmitBatch`) with an optional
   `fee_payer: Option<FeeAuthorization>`, where `FeeAuthorization {
   payer_pubkey_hash: [u8; 32], fee_signature: Vec<u8>, max_fee:
   Balance }`. Append-only, so pre-fee clients still decode (the same
   append-only discipline PERF-2 used for `SubmitFastPath`).
2. **A third domain-tagged digest** — `client.rs`. Add
   `FEE_DOMAIN_TAG = b"SUWAPPU_FEE_V1"` and `fee_signing_digest(
   network_id, intent_hash, max_fee)` mirroring `intent_signing_digest`
   / `fastpath_signing_digest`. Bind it to the intent *hash* (not the
   intent bytes) + `max_fee` so the sponsor authorizes one specific
   intent at a capped price.
3. **`verify_signed_fee`** — `client.rs`. Mirror `verify_signed_intent`
   / `verify_signed_fastpath`, reusing `verify_authority_signature`
   (`client.rs:470`). Note the current Authority-Ring-only gate: a
   non-Authority fee payer needs the Issue #28 auth extension first.
4. **Fee-derived mempool priority** — `client.rs:510` + the
   `state.mempool.submit(...)` call sites. Retire
   `DEFAULT_INTENT_PRIORITY`; pass `max_fee` (or a fee/size ratio) as the
   `priority` argument. `Mempool::submit`'s signature
   (`mempool.rs:146`, opaque `priority: u64`) and the `EntryKey`
   ordering (`mempool.rs:97`) are unchanged — the knob already exists.
5. **Fee deduction in the execute path** —
   [`crates/suwappu-execution/src/substrate.rs`](../../crates/suwappu-execution/src/substrate.rs)
   `Substrate::apply` (both `InMemorySubstrate` and `SuwappuDbSubstrate`)
   and [`block.rs`](../../crates/suwappu-execution/src/block.rs)
   `execute_block`. Before/after each intent's effect, debit the fee
   from `fee_payer` (or the intent's `from`) and credit a reward
   recipient (a `reserved.rs` address). The debit+credit joins the
   intent's bundle so it is atomic and dual-VM-consistent with it. This
   does **not** change the joint-quorum commit path or the checkpoint
   struct.

**Phase 2 (Option B), gated:**

6. **Precompile dispatch** — wire `issuer` / `reserve`
   (`crates/suwappu-precompiles/`) into `apply_intent`, the same
   integration Track G's L2 verifier precompile requires (IQ-006).
   Reference the issuer `AssetId` (`issuer.rs:42`) as the fee-token
   identifier.
7. **A fee-AMM Intent arm** — a new `#[non_exhaustive]` `Intent` variant
   (e.g. `ConvertFee { .. }`) converting the payer's fee asset into the
   validator-preferred asset, with the rate source treated as its own
   trust-surface decision (needs its own IQ). Reserve-coverage pauses on
   the fee asset must degrade gracefully, not brick the fee market.

## Open questions

- **Fee unit while SUWAPPU-only.** Flat priority tip, or gas×price? No
  gas metering exists anywhere today, so a metered model is itself net-new
  work; a flat `max_fee` tip is the minimum viable Phase-1 unit.
- **Fee recipient.** Proposer, `treasury_address`, or burn — and how
  that composes with `MintInflation` / `DistributeRewards` tokenomics.
- **Sponsorship replay.** Binding the fee digest to the intent content
  hash blocks cross-intent replay; same-intent resubmission still needs
  the mempool dedup or an explicit nonce. Does the fee payer need a nonce
  account?
- **Auth surface.** Fee payers who are not seated Authorities require the
  Issue #28 extension (Validator/user keys are not carried in a registry
  today). Until then a "fee payer" can only be another Authority — which
  is fine for a demo, not for real sponsorship.
- **Commit-path independence (invariant 1).** Confirm fee failure at
  execution (insufficient `fee_payer` balance) is handled *post-commit*
  as an intent-level revert, never as a reason to withhold or re-order a
  joint-quorum commit. A fee mechanism that gates commit on payment
  couples the two rings and must be rejected.
- **ML-DSA-65 size budget.** Measure the ~6.6 KB two-signature sponsored
  tx against `MAX_FRAME_BYTES` and mempool capacity before locking the
  always-detached-signature design; consider whether the sponsor
  signature can be aggregated or referenced rather than inlined per tx.
- **Fast-path interaction.** Fast-path txs bypass the mempool (they
  gossip as partial certs, `client.rs:718` ff). Are sponsored fees in
  scope for the fast-path lane, or main-lane only in Phase 1?
- **Multi-asset numeraire.** Phase 2 multi-asset fees need a common unit
  to order the mempool — reintroducing the fee-AMM oracle at admission
  time.

## Decision

**Pending sign-off.**

## See also

- [`docs/research/competitive-gap-analysis.md`](../research/competitive-gap-analysis.md)
  — FEE-1 / gap G-2 (§3 register, §6 P1 workstream).
- [`docs/research/briefs/tempo.md`](../research/briefs/tempo.md) §3 —
  fee-payer/fee-token selector, fee AMM, second-signature sponsorship.
- [`docs/research/briefs/arc.md`](../research/briefs/arc.md) §3 —
  USDC-gas EWMA fees + Circle Paymaster.
- [`crates/suwappu-execution/src/substrate.rs:103`](../../crates/suwappu-execution/src/substrate.rs)
  — the `#[non_exhaustive]` `Intent` enum (no fee field today).
- [`crates/suwappu-node/src/client.rs:125,145,510`](../../crates/suwappu-node/src/client.rs)
  — the two signing digests and `DEFAULT_INTENT_PRIORITY = 0`.
- [`crates/suwappu-mempool/src/mempool.rs:97,146`](../../crates/suwappu-mempool/src/mempool.rs)
  — the opaque-`priority` admission key.
- [`crates/suwappu-precompiles/src/issuer.rs`](../../crates/suwappu-precompiles/src/issuer.rs),
  [`reserve.rs`](../../crates/suwappu-precompiles/src/reserve.rs)
  — issuer mint/burn + reserve-coverage primitives (not
  `apply_intent`-dispatched today).
- [IQ-006](./IQ-006-l2-state-root-commitment-surface.md) — the
  "precompiles are standalone, not dispatched by `apply_intent`" finding
  that Phase 2 shares.
</content>
</invoke>
