# IQ-007 — Governance authorization as a consensus rule

**Status:** Recommendation, implemented on branch
`claude/lattice-chain-quality-parity-x78o7d`. Hardened across five
adversarial consensus-review rounds (each closed a real finding:
dynamic-peer unsigned Vote/Block; envelope not bound to the signed cert;
block-availability first-write-wins poison; commit-order defer scope on
the block axis; and the identical defer scope on the validator-vote axis
— `if !stake_ok { break 'commit }`).

**Automated coverage for the core property** (every honest node commits a
strictly-growing prefix of the joint-gated finalize order under selective
block/vote unavailability) is now split across two layers:

- The **pure-consensus** append-only ordering property runs at 10k in
  `crates/suwappu-consensus/tests/proptest_dagbft_commit.rs`
  (`finalize_is_append_only`).
- The **daemon-level** defer-under-unavailability + `GetBlock` recovery
  guarantee — the piece the pure layer cannot see — is covered by
  `phase_g_growing_prefix_under_transient_unavailability` (inline in
  `crates/suwappu-node/src/daemon.rs`). It drives a real 4-node loopback
  cluster (which naturally produces transient block/vote gaps and
  recovery) through a multi-round mix of transfers and a governed
  `AdmitAuthority`, asserting: (1) no node ever rewrites a round it has
  already finalized, (2) nodes never disagree on a commonly-held round
  (no fork of the joint order), and (3) every node makes the identical
  governance apply decision. A full 10k `proptest!` is impractical at the
  daemon layer (a real multi-node tokio cluster per case), so this is a
  deterministic scenario rather than a fuzzed one.

**This automated coverage is necessary but NOT sufficient.** Because this
touches BFT commit ordering and block availability on a settlement chain
and repeatedly surfaced subtle safety issues under review, it MUST NOT be
treated as production-ready without human consensus-team sign-off and a
loaded multi-node devnet run with adversarial fault injection
(block-withholding / stripped-block relay / straggler-cert ordering), on
top of the tests above. The subagent reviews and CI are necessary but not
sufficient for a change of this class.
**Owner:** consensus
**Date:** 2026-08-15
**Tracking:** external-validator-join effort; supersedes the
Issue #28 "dual-signature deferred" note in `crates/suwappu-node/src/client.rs`.

## Question

How should authority-management intents (`AdmitAuthority`,
`ExitAuthority`, `EjectAuthority`) be authorized so that no single party
can reshape the validator set — and so that the authorization is
enforced by consensus, not merely by the ingress API?

## Background

Before this change, governance intents were authorized only at ingress:
the client wire (`SubmitGoverned`) and JSON-RPC verified a signature and
then discarded it. `Intent` carries no signatures, so once an intent was
in a committed block, `apply_governance_intent` mutated the Authority and
Validator registries at the epoch boundary with **no authorization
re-check**.

An adversarial consensus review of the initial dual-signature work
identified two independent single-party paths to controlling both rings —
the exact failure class Load-bearing Invariant 1 (joint-quorum AND-gate,
Paper Theorem 2) exists to exclude:

1. **Block-author bypass.** A Byzantine *seated* authority skips the
   client wire entirely: it authors its own validly-signed cert whose
   block contains an un-cosigned `AdmitAuthority`. Honest nodes accept
   the cert (author is seated, signature valid), the leader-commit sweeps
   it into causal history, and every honest node admits the attacker's
   candidate with attacker-chosen stake. The ingress gate never runs.

2. **Candidate self-co-sign.** Even at the gate, the original
   `AdmitAuthority` co-signer was the *candidate* — a key the attacker
   generates. One compromised seated key could sponsor-sign an admit for
   its own candidate key and co-sign it as that candidate: fully
   gate-compliant, single party.

## Decision

Governance authorization becomes a **commit-time consensus rule** carried
on-chain, with a two-distinct-authority requirement for every governance
action.

### Envelope (`GovAuth`, `crates/suwappu-node/src/client.rs`)

Each governance intent carries an authorization envelope:

- `sponsor` — a seated Authority Ring member; ML-DSA-65 signature over
  `intent_signing_digest(network_id, intent)`.
- `co_signer` — a **second, distinct** seated Authority Ring member;
  signature over the same digest. Required for `AdmitAuthority` too, not
  only Exit/Eject.
- `candidate_pop` — `AdmitAuthority` only: the admitted key signs the
  same digest (proof of possession, so an admit cannot be forged for a
  key nobody holds). This is *in addition to*, not a substitute for, the
  second authority.

Both authority signatures bind the same digest, so the domain-separation
(`SUWAPPU_INTENT_V1`) and `network_id` replay defenses apply to each.

### On-chain carriage and re-verification

- The envelope travels inside `BlockPayload.governance_auth`, keyed by
  the governance intent's index within `intents`. The authoring node
  attaches the envelope it verified at ingest (retained in a bounded
  `State` map, consumed at block build).
- **The envelope is bound into the signed cert.** `payload_digest` is
  computed by `compute_payload_digest(intents, governance_auth)` — over
  BOTH the intents and the envelopes — and the `Certificate` commits to
  and signs that digest. `block_payload_is_consistent` recomputes it, so
  a relayed block with a stripped or mutated envelope has a different
  digest and is rejected at ingest; and `try_commit` only consumes a
  block whose `payload_digest` equals the committed cert's signed
  `payload_digest`. A `Block` frame from an unauthenticated dynamic peer
  is dropped outright.
- `apply_governance_intent` re-verifies the envelope against **this
  node's** seated Authority Ring and the manifest `network_id` before any
  registry mutation, and drops the intent if the envelope is missing or
  invalid.

### Determinism argument

Every honest node that commits a given cert has, by cert-binding above,
the byte-identical `governance_auth` the author signed — the envelope is
covered by the cert signature, not merely delivered alongside it. At a
given epoch boundary every honest node has committed the same certs (leader-commit
total order) and holds the same Authority Registry and manifest
`network_id`. `verify_governed_intent` is a pure function of (intent,
envelope, registry, network_id), and the drain order is the deterministic
commit order. Therefore all honest nodes make the identical apply/drop
decision for each queued governance intent — no state divergence. A
Byzantine block author gains nothing by forging or omitting the envelope
(verified, not trusted → deterministic drop everywhere), and a Byzantine
relay cannot strip the envelope from a committed cert's block (digest
mismatch → rejected).

> Design-history note: the initial implementation (commit b6c60ad) carried
> `governance_auth` in the block but did NOT bind it into `payload_digest`.
> A consensus review correctly found that a relay could strip the envelope
> from one copy of an otherwise-identical block, splitting honest nodes
> into apply-vs-drop and diverging the registries. The digest binding and
> cert-binding above were added in response and are load-bearing for this
> argument.

## Consequences

- **Closed:** both single-party paths above. Reshaping the validator set
  now requires two distinct seated Authority Ring members, enforced at
  commit. A leaked client signing key alone, or a single Byzantine
  authoring node alone, is insufficient.
- **Wire:** `CLIENT_WIRE_VERSION = 3`. `BlockPayload` gains a
  `#[serde(default)]` `governance_auth` field (older blocks decode with
  an empty vector — and any governance intent in such a block is dropped,
  which is the safe direction).
- **Not addressed here (future work):**
  - Stake in `AdmitAuthority` is still a claimed integer, not an escrowed
    bond. A two-authority collusion can still admit with arbitrary
    declared stake; bonding is a separate change.
  - BLS material in the admit envelope is still discarded by the daemon
    (as at genesis); BLS aggregation of the co-signatures is a possible
    future compaction.
  - The Validator Ring has no independent join path
    (`AdmitAuthority` mirrors into both registries).

## Alternatives considered

- **Embed the signatures in the `Intent` enum.** Cleaner conceptually
  (auth intrinsic to the intent) but ripples through ~40 construction
  sites and both apply paths (daemon registry + substrate stake records)
  across crates; higher risk for no security gain over the
  `BlockPayload` envelope.
- **Keep it an ingress-only gate.** Rejected: does not bind a Byzantine
  block author, i.e. does not make the guarantee a consensus property.
- **Require a full quorum co-sign.** Stronger, but a two-authority
  threshold already defeats the single-party attack and keeps the
  operational path (foundation + one other seat) tractable for the
  testnet; the threshold can be raised later without a wire change.
