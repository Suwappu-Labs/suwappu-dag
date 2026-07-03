# IQ-011 — Travel-rule messaging (originator / beneficiary VASP data)

**Status:** Recommendation, pending sign-off.
**Owner:** compliance / execution
**Date:** 2026-07-03
**Tracking:** Refs [`docs/research/compliance-regime-mapping.md`](../research/compliance-regime-mapping.md)
§4 (the prime missing hook) + competitive gap **G-7 / COMP-1**. Cross-refs gap
**G-4** (no issuer / VASP story — travel-rule is primitives-not-a-program and
only bites under a regulated CASP).

## Question

The compliance-regime mapping (COMP-1) confirmed against the code that
suwappu-dag has **no travel-rule surface**: originator/beneficiary VASP identity
on CASP-to-CASP transfers — the EU TFR / FATF "travel rule" — has nowhere to
live. Under EU TFR the threshold is **zero-value** (brief
[`landscape.md`](../research/briefs/landscape.md) §5, §2): every transfer between
crypto-asset service providers must carry originator + beneficiary data, no de
minimis. So this is the common path for any regulated-CASP deployment, not an
edge case, and the GENIUS AML/sanctions NPRMs (FinCEN/OFAC, April 2026, §5)
push the same direction on the US side by treating permitted-payment-stablecoin
issuers as BSA financial institutions.

**How should suwappu-dag carry travel-rule data (originator + beneficiary VASP
identity) for regulated transfers — an on-chain attachment to the transfer
Intent, an off-chain TRISA/IVMS-101 messaging layer keyed to the on-chain tx, or
a hybrid — without bloating the transaction or leaking PII on-chain?**

Three options were surveyed. None maps onto an existing data structure without
modification.

### Current state (grounded in the code, 2026-07-03)

- **The transfer surface carries no messaging channel.** `Intent::Transfer {
  from, to, amount }` at
  [`crates/suwappu-execution/src/substrate.rs:103-112`](../../crates/suwappu-execution/src/substrate.rs)
  is three fields, all value-movement — **no memo, note, attachment, reference,
  or originator/beneficiary field**, and there is no separate travel-rule
  message Intent variant. There is no on-chain commitment surface (hash/root)
  that a transfer could bind an off-chain IVMS-101 payload to.
- **The submission envelope is content-hashed and signed but has no payload
  slot.** `ClientMessage::Submit(Intent)`
  ([`crates/suwappu-node/src/client.rs`](../../crates/suwappu-node/src/client.rs))
  wraps a bare `Intent`; the tx hash is `blake3(bincode(intent))` and the
  signing digest is `intent_signing_digest(network_id, intent)`. There is no
  side-channel or attachment field on the envelope — a travel-rule payload has
  nowhere to ride except inside the Intent itself.
- **The DID resolver already models an addressable endpoint but no payload.**
  `Service { id, service_type, endpoint }`
  ([`crates/suwappu-precompiles/src/did.rs:93-103`](../../crates/suwappu-precompiles/src/did.rs))
  is a free-form `service_type` + `endpoint` string on a `DidDocument`. A CASP
  DID could advertise a Notabene/TRISA/Veriscope travel-rule endpoint via a
  `Service` entry — the **identity/addressing** layer exists; the **payload and
  its on-chain binding** do not. DID validation is structural only (no
  credential verification, no revocation), so a DID today asserts an address,
  not a licensed VASP.
- **The reserved-registry-account pattern already exists** for exactly this
  kind of precompile-owned commitment store: `reserved::is_reserved` gates
  user `Transfer`s out of registry accounts
  ([`substrate.rs:1599-1612`](../../crates/suwappu-execution/src/substrate.rs)),
  the same pattern IQ-006 uses for the L2 state-root registry.

## Options surveyed

### Option A — Off-chain TRISA/IVMS-101 messaging, only a commitment on-chain (RECOMMENDED)

The IVMS-101 originator/beneficiary payload is exchanged **VASP-to-VASP
off-chain** over an existing travel-rule protocol (TRISA / Notabene / Veriscope
per brief §2 item 4). The chain carries only a **32-byte commitment** —
`SHA3-256(IVMS-101 payload || tx_hash)` — that binds a specific transfer to a
specific VASP-to-VASP message. The commitment lives in a precompile-owned
**reserved travel-rule registry account** (the IQ-006 pattern), keyed by
`tx_hash`; the transfer Intent itself is unchanged. Originator/beneficiary VASPs
resolve to each other's messaging endpoints via the DID `Service` layer (see
Option C, which A subsumes as its addressing mechanism).

**Pros:**
- **No PII on-chain.** Only a hash is committed; names, account numbers, and
  addresses never touch the ledger. This is MiCA/GDPR-friendly (immutable-ledger
  PII is a live GDPR problem) and matches how the industry actually does it —
  TRISA/Notabene/Veriscope are all off-chain messaging rails with on-chain
  settlement, not on-chain payload carriers (brief §2, §5).
- **Zero bytes added to the transaction.** `Intent::Transfer` stays
  `{ from, to, amount }`; the commitment is written by a dedicated precompile
  arm into a reserved account, so the hot-path transfer wire format, its
  `blake3(bincode(intent))` content hash, and the IQ-005 wire-frame version
  marker are all untouched. No `#[non_exhaustive]` churn across the five Intent
  consumers (`suwappu-execution`, `suwappu-node`, `suwappu-rpc`,
  `suwappu-fastpath`, `suwappu-mempool`).
- **Tamper-evident binding without custody of PII.** The commitment lets an
  auditor/regulator later prove that the on-chain transfer corresponds to the
  exact IVMS-101 message the VASPs exchanged — the chain's job is
  non-repudiation of the linkage, not storage of the data.
- **Reuses two patterns already in the tree**: the reserved-registry-account
  (IQ-006) and the DID `Service` endpoint for VASP addressing.

**Cons:**
- **Concedes that travel-rule compliance lives off-chain.** For an
  institutional-settlement pitch this is a positioning choice worth stating: the
  chain provides a binding, not a compliance program. Honest, but a competitor
  with on-chain confidential-but-auditable transfers (Solana Token-2022, Sui,
  Canton — brief §2 item 3) can claim a tighter integration.
- **Requires an off-chain messaging integration** (TRISA membership /
  Notabene/Veriscope API) that suwappu-dag does not own and cannot enforce. The
  chain can require the commitment to be present; it cannot verify the payload
  behind it was well-formed IVMS-101. Enforcement ("no transfer without a valid
  travel-rule message") is a policy the operating CASP runs off-chain.
- **Still adds one commitment write** per regulated transfer — cheap, but it is
  a new precompile dispatch arm and a new reserved account to reserve against
  collision.

### Option B — Encrypted on-chain attachment (ML-KEM-768-sealed IVMS-101)

Add an optional attachment to the transfer path: an **ML-KEM-768-sealed**
IVMS-101 payload, readable by the beneficiary VASP and by a regulator viewing
key. This either extends `Intent::Transfer` with an `Option<TravelRuleBlob>`
field or introduces a paired `Intent::TravelRuleAttach { tx_hash, sealed_blob }`
variant on the `#[non_exhaustive]` enum.

**Pros:**
- **Ties directly to our PQ confidentiality surface.** ML-KEM-768 (FIPS 203) is
  already load-bearing invariant 2 and shipped in `suwappu-crypto` — sealing the
  payload to the beneficiary VASP + regulator viewing key is a natural use of a
  primitive we already have, and it is a genuine differentiator (no payments
  chain ships NIST PQ as its primary surface — brief §3). Confidential-but-
  auditable is exactly the 2026 table-stakes pattern (brief §2 item 3).
- **Self-contained.** No off-chain messaging membership required; the data
  travels with the settlement, so there is no reconciliation gap between an
  off-chain message and the on-chain tx.

**Cons:**
- **Adds per-tx bytes — directly in tension with a lean transaction.** An
  IVMS-101 record plus an ML-KEM-768 ciphertext (~1,568 B for the KEM alone,
  before the AEAD-wrapped payload) is a large attachment on a hot-path transfer.
  This is the same "don't add per-payload on-chain bytes" pressure that
  invariant 3 imposes on LTP commitments; a transfer is far higher-frequency
  than an LTP attestation, so the cost is worse.
- **Encrypted-but-immutable PII is still PII on-chain — a GDPR liability.**
  Ciphertext of personal data is personal data under GDPR; an immutable ledger
  cannot honor erasure ("right to be forgotten"), and a future cryptographic
  break (or a leaked viewing key) retroactively exposes every historical
  payload. This is the exact risk the constant-size/off-chain philosophy exists
  to avoid.
- **Touches the load-bearing Intent surface.** Whether as a `Transfer` field or
  a new variant, it changes the highest-traffic execution path — lane
  separation, determinism, bundle atomicity (invariant 4) all in scope — and
  ripples `#[non_exhaustive]` matches across all five consumer crates plus both
  SDKs. Bloats the fast-path single-owner lane specifically.
- **Regulator-viewing-key management is a new trust surface** (key rotation,
  jurisdiction, custody) with no home in the current design.

### Option C — DID-anchored identity, travel-rule data off-chain against DIDs

Originator and beneficiary each resolve to a `did:suwappu:<id>` via the existing
DID precompile; the transfer references those DIDs (or the parties are already
DID-bound), and IVMS-101 data flows off-chain **against those on-chain
identities** — a VASP looks up the counterparty's travel-rule `Service` endpoint
on its DID document and messages it directly. Nothing travel-rule-specific is
committed on-chain beyond what the DID layer already stores.

**Pros:**
- **Zero new on-chain surface.** Leaves both `Intent::Transfer` and the
  submission envelope completely untouched; the only thing on-chain is the DID
  `Service` endpoint that already exists (`did.rs:93-103`). Lowest-risk,
  lowest-churn option.
- **Correct addressing primitive.** DID `Service` endpoints are the right way to
  advertise a VASP's Notabene/TRISA/Veriscope messaging endpoint, and this is
  precisely the mechanism Option A relies on for discovery — so C is not a rival
  design so much as A's addressing layer without the binding commitment.

**Cons:**
- **No binding between the transfer and the travel-rule message.** Without a
  commitment (Option A) there is nothing on-chain tying a specific `tx_hash` to
  a specific IVMS-101 exchange — an auditor cannot later prove the linkage from
  chain data alone. This is the decisive weakness: identity-addressing is
  necessary but not sufficient for travel-rule non-repudiation.
- **DID today asserts an address, not a licensed VASP.** DID validation is
  structural only — no verifiable-credential check, no revocation registry
  (`did.rs`, per COMP-1 §2). Anchoring travel-rule on DIDs presumes a VASP-
  attestation layer on top of the DID that does not exist yet (gap G-4 again).
- **Not zero-threshold-safe on its own.** EU TFR requires the data to
  *accompany* the transfer; C provides discovery but no evidence the exchange
  happened for a given transfer, which is what a supervisor asks for.

## Decision

**Option A — off-chain TRISA/IVMS-101 messaging with a 32-byte on-chain
commitment, using the DID `Service` layer (Option C's mechanism) for VASP
addressing.**

A keeps PII off-chain (MiCA/GDPR-friendly), adds zero bytes to the hot-path
transfer, matches how the travel-rule industry actually operates
(TRISA/Notabene/Veriscope are off-chain rails), and still gives a regulator a
tamper-evident binding from `tx_hash` to the VASP-to-VASP message. It composes
C (DID `Service` endpoints for discovery) rather than competing with it, and it
deliberately declines B's encrypted-on-chain-attachment because encrypted-but-
immutable PII is a standing GDPR liability and the per-tx bytes fight a lean
transfer and the fast-path lane.

**This is primitives, not a program (gap G-4).** The commitment surface is
useful only under an operating, regulated CASP/VASP that runs the off-chain
IVMS-101 exchange and enforces "no regulated transfer without a travel-rule
message." suwappu-dag has no issuer/VASP today; the chain provides the binding,
the VASP provides the compliance obligation and the payload. Nothing here is a
compliance opinion or a discharged legal duty.

### Implementation sketch

Concrete types, mirroring the IQ-006 reserved-registry pattern so the transfer
Intent and its wire format stay untouched:

1. **Reserved travel-rule registry account.** Derive
   `TRAVEL_RULE_REGISTRY_ADDRESS = BLAKE3("suwappu-travel-rule-registry-v1")[..20]`,
   reserved via the existing `reserved::is_reserved` gate
   ([`substrate.rs:1599-1612`](../../crates/suwappu-execution/src/substrate.rs))
   so no user `Transfer` can mutate it. Owned by a travel-rule precompile arm,
   not user-spendable.

2. **Commitment shape.** Store a map `tx_hash → TravelRuleCommitment`:

   ```rust
   pub struct TravelRuleCommitment {
       /// SHA3-256(ivms101_payload || tx_hash) — binds the off-chain
       /// VASP-to-VASP message to this exact transfer. No PII on-chain.
       pub payload_commitment: [u8; 32],
       /// Originator VASP DID (resolves to a travel-rule Service endpoint).
       pub originator_vasp: Did,
       /// Beneficiary VASP DID.
       pub beneficiary_vasp: Did,
       /// L1 height at which the commitment was recorded.
       pub committed_at_l1_height: u64,
   }
   ```

   The commitment uses SHA3-256 for consistency with the paper's payload-root
   convention (invariant 3) and the `sha3_256_domain` helper in
   [`crates/suwappu-crypto/src/hash.rs`](../../crates/suwappu-crypto/src/hash.rs).

3. **A new dispatch arm — not a `Transfer` field.**
   `Intent::AttachTravelRule { tx_hash, payload_commitment, originator_vasp,
   beneficiary_vasp }` on the `#[non_exhaustive]` Intent enum
   ([`substrate.rs:103`](../../crates/suwappu-execution/src/substrate.rs)),
   dispatched to the travel-rule registry write. This keeps `Intent::Transfer`
   byte-for-byte unchanged and leaves the fast-path single-owner lane
   untouched; the commitment rides the existing IQ-005 wire-frame and inherits
   the `blake3(bincode(intent))` content-hash recipe without modification. Per
   CLAUDE.md, validate the `#[non_exhaustive]` propagation with
   `cargo check -p suwappu-execution -p suwappu-node -p suwappu-rpc
   -p suwappu-fastpath -p suwappu-mempool`.

4. **VASP addressing via DID `Service`.** Define a canonical
   `service_type = "TravelRuleMessaging"` convention on the DID `Service` struct
   ([`did.rs:93-103`](../../crates/suwappu-precompiles/src/did.rs)); the
   `endpoint` carries the TRISA/Notabene/Veriscope URL. No schema change to
   `Service` — it is a `service_type` string convention plus documentation.

5. **Reader API.** Expose
   `suwappu_getTravelRuleCommitment { tx_hash } → TravelRuleCommitment` from
   `crates/suwappu-rpc` for auditor/regulator verification of the linkage.

### What this decision does NOT change

- `Intent::Transfer { from, to, amount }` stays exactly as it is
  ([`substrate.rs:103-112`](../../crates/suwappu-execution/src/substrate.rs)).
  No memo, no attachment, no per-tx bytes; the fast path is untouched.
- The submission envelope
  ([`client.rs`](../../crates/suwappu-node/src/client.rs)) is unchanged; the
  commitment rides an ordinary Intent through the existing signed path.
- The DID `Service` struct is unchanged — only a `service_type` string
  convention is added.

## Open questions

1. **Is the commitment mandatory or advisory?** The chain can *require* an
   `AttachTravelRule` alongside a `Transfer` for flagged CASP accounts, or leave
   presence to off-chain policy. Enforcement-in-consensus vs. off-chain-policy
   is a substrate-invariant question (does a missing commitment reject the
   transfer?) and depends on the issuer/VASP model (gap G-4).
2. **One commitment per transfer, or batched?** High-volume corridors may want a
   per-batch travel-rule root rather than per-tx commitment.
3. **VASP attestation on the DID.** Option A presumes originator/beneficiary
   DIDs are *licensed VASPs*, but DID validation is structural only. Does this
   need a verifiable-credential / VASP-registry layer on the DID precompile
   first (interacts with the registered-issuer precompile)?
4. **Does GENIUS AML (US) accept the same commitment surface**, or does FinCEN's
   BSA-institution framing want a different retention/reporting shape than EU
   TFR? NPRMs are proposed, not final (effective ~2027); revisit on finalization.
5. **Regulator access model** for the off-chain payload behind the commitment —
   who holds it, for how long, under which jurisdiction's retention rules.

## Decision

**Pending sign-off.**

## See also

- [`docs/research/compliance-regime-mapping.md`](../research/compliance-regime-mapping.md) §4 —
  the missing-hook analysis this IQ closes (COMP-1 / gap G-7).
- [`docs/research/briefs/landscape.md`](../research/briefs/landscape.md) §2 item 4, §5 —
  EU TFR zero-value threshold, GENIUS AML/sanctions NPRMs, Notabene/TRISA/Veriscope.
- [`crates/suwappu-execution/src/substrate.rs:103-112`](../../crates/suwappu-execution/src/substrate.rs) —
  the `Intent::Transfer` surface that stays unchanged.
- [`crates/suwappu-execution/src/substrate.rs:1599-1612`](../../crates/suwappu-execution/src/substrate.rs) —
  the reserved-registry-account gate reused for the travel-rule registry.
- [`crates/suwappu-precompiles/src/did.rs:93-103`](../../crates/suwappu-precompiles/src/did.rs) —
  the DID `Service` endpoint used for VASP addressing.
- [`crates/suwappu-node/src/client.rs`](../../crates/suwappu-node/src/client.rs) —
  the submission envelope (`ClientMessage::Submit(Intent)`, `blake3(bincode)` tx hash).
- [IQ-006](./IQ-006-l2-state-root-commitment-surface.md) — the reserved-
  registry-account + commitment pattern this IQ mirrors.
